//! Owner-only accounting and recovery cleanup. Bases and current disks are never targets.
use crate::state::{self, Session};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::{self, File, Metadata},
    io::{Read, Seek, SeekFrom},
    os::unix::{ffi::OsStringExt, fs::MetadataExt},
    path::{Path, PathBuf},
};

const CATEGORIES: [&str; 5] = ["images", "sessions", "trash", "media", "cache"];
const MAX_ENTRIES: usize = 100_000;

#[derive(Default, Serialize)]
struct Usage {
    logical_bytes: u64,
    allocated_bytes: u64,
    files: u64,
}

fn real(path: &Path, directory: bool) -> Result<Metadata> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        !meta.file_type().is_symlink() && meta.is_dir() == directory,
        "unexpected type or symlink: {}",
        path.display()
    );
    if !directory {
        ensure!(meta.is_file(), "not a regular file: {}", path.display());
    }
    Ok(meta)
}

// Do not follow symlinks or cross filesystem mounts; deduplicate hard links.
fn walk(
    path: &Path,
    files: &mut Vec<(PathBuf, Metadata)>,
    depth: usize,
    device: u64,
    visited: &mut usize,
) -> Result<()> {
    *visited += 1;
    ensure!(
        depth < 32 && *visited <= MAX_ENTRIES,
        "storage inventory limit exceeded"
    );
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.dev() == device,
        "storage contains another filesystem: {}",
        path.display()
    );
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            walk(&entry?.path(), files, depth + 1, device, visited)?;
        }
    } else {
        files.push((path.to_owned(), meta));
    }
    Ok(())
}

fn inventory(root: &Path) -> Result<Vec<(PathBuf, Metadata)>> {
    let device = real(root, true)?.dev();
    let mut files = Vec::new();
    let mut visited = 0;
    for category in CATEGORIES {
        let dir = root.join(category);
        if fs::symlink_metadata(&dir).is_ok() {
            real(&dir, true)?;
            walk(&dir, &mut files, 0, device, &mut visited)?;
        }
    }
    Ok(files)
}

fn usage(files: &[(PathBuf, Metadata)], path: &Path) -> Usage {
    let mut result = Usage::default();
    let mut seen = HashSet::new();
    for (file, meta) in files {
        if file.starts_with(path) && seen.insert((meta.dev(), meta.ino())) {
            result.logical_bytes += meta.len();
            result.allocated_bytes += meta.blocks() * 512;
            result.files += 1;
        }
    }
    result
}

fn uuid(value: &str) -> Result<()> {
    ensure!(
        uuid::Uuid::parse_str(value)?.to_string() == value,
        "expected a canonical UUID"
    );
    Ok(())
}

fn trash_name(value: &str) -> Result<&str> {
    ensure!(value.is_ascii() && value.len() > 37, "invalid trash target");
    let split = value.len() - 37;
    ensure!(&value[split..split + 1] == "-", "invalid trash target");
    state::validate_name(&value[..split])?;
    uuid(&value[split + 1..])?;
    Ok(&value[..split])
}

fn resolve(root: &Path, target: &str) -> Result<(PathBuf, PathBuf, bool, String)> {
    let parts: Vec<_> = target.split('/').collect();
    match parts.as_slice() {
        ["trash", slot] => {
            let name = trash_name(slot)?.to_owned();
            real(&root.join("trash"), true)?;
            let dir = root.join("trash").join(slot);
            real(&dir, true)?;
            Ok((dir.clone(), dir, true, name))
        }
        ["recovery", name, id] => {
            state::validate_name(name)?;
            uuid(id)?;
            real(&root.join("sessions"), true)?;
            let dir = root.join("sessions").join(name);
            real(&dir, true)?;
            let file = dir.join(format!("system-before-reset-{id}.qcow2"));
            real(&file, false)?;
            Ok((file, dir, false, (*name).to_owned()))
        }
        _ => bail!("use one exact trash/NAME-UUID or recovery/SESSION/UUID target from storage"),
    }
}

pub fn report(root: &Path) -> Result<Value> {
    let _guard = state::storage_lock(root, false)?;
    let root = fs::canonicalize(root)?;
    let files = inventory(&root)?;
    let categories: Vec<_> = CATEGORIES
        .iter()
        .map(|name| json!({"category":name,"usage":usage(&files, &root.join(name))}))
        .collect();
    let mut targets = Vec::new();
    let trash = root.join("trash");
    if trash.is_dir() {
        for entry in fs::read_dir(&trash)? {
            let path = entry?.path();
            if let Some(slot) = path.file_name().and_then(|s| s.to_str())
                && trash_name(slot).is_ok()
                && real(&path, true).is_ok()
            {
                targets.push(json!({"target":format!("trash/{slot}"),"usage":usage(&files,&path)}));
            }
        }
    }
    for (path, meta) in &files {
        if !meta.is_file() {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(root.join("sessions")) {
            let parts: Vec<_> = relative.iter().collect();
            if parts.len() == 2
                && let (Some(name), Some(file)) = (parts[0].to_str(), parts[1].to_str())
                && state::validate_name(name).is_ok()
                && let Some(id) = file
                    .strip_prefix("system-before-reset-")
                    .and_then(|s| s.strip_suffix(".qcow2"))
                && uuid(id).is_ok()
            {
                targets.push(
                    json!({"target":format!("recovery/{name}/{id}"),"usage":usage(&files,path)}),
                );
            }
        }
    }
    targets.sort_by(|a, b| a["target"].as_str().cmp(&b["target"].as_str()));
    Ok(
        json!({"categories":categories,"total":usage(&files,&root),"recovery_targets":targets,
        "accounting":"file lengths and allocated blocks; hard links counted once per group; no symlink traversal; not a reclaim estimate",
        "cleanup":"prune TARGET previews; prune TARGET --execute permanently removes that recovery copy"}),
    )
}

// Read only the QCOW2 backing-name header, never image contents. Unknown/external
// data formats fail closed rather than silently losing a dependency.
fn backing(path: &Path) -> Result<Option<PathBuf>> {
    real(path, false)?;
    let mut file = File::open(path)?;
    let mut header = [0u8; 80];
    file.read_exact(&mut header[..72])?;
    ensure!(
        &header[..4] == b"QFI\xfb",
        "invalid qcow2 header: {}",
        path.display()
    );
    let version = u32::from_be_bytes(header[4..8].try_into()?);
    ensure!(version == 2 || version == 3, "unsupported qcow2 version");
    if version == 3 {
        file.read_exact(&mut header[72..])?;
        let flags = u64::from_be_bytes(header[72..80].try_into()?);
        ensure!(
            flags & !9 == 0,
            "unsupported or corrupt qcow2 features: {}",
            path.display()
        );
    }
    let offset = u64::from_be_bytes(header[8..16].try_into()?);
    if offset == 0 {
        return Ok(None);
    }
    let size = u32::from_be_bytes(header[16..20].try_into()?) as usize;
    ensure!(
        size > 0
            && size <= 1023
            && offset
                .checked_add(size as u64)
                .is_some_and(|end| end <= file.metadata().map(|m| m.len()).unwrap_or(0)),
        "invalid qcow2 backing name"
    );
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; size];
    file.read_exact(&mut bytes)?;
    ensure!(!bytes.contains(&0), "invalid backing name");
    let name = PathBuf::from(std::ffi::OsString::from_vec(bytes));
    Ok(Some(fs::canonicalize(
        path.parent().context("image parent")?.join(name),
    )?))
}

fn check_dependencies(files: &[(PathBuf, Metadata)], target: &Path) -> Result<()> {
    for (path, meta) in files {
        if path.starts_with(target) || path.extension().is_none_or(|s| s != "qcow2") {
            continue;
        }
        ensure!(meta.is_file(), "unexpected qcow2 alias: {}", path.display());
        let mut next = path.clone();
        let mut seen = HashSet::new();
        for depth in 0..32 {
            ensure!(
                depth < 31 && seen.insert(next.clone()),
                "backing chain too deep or cyclic"
            );
            match backing(&next)
                .with_context(|| format!("checking dependency {}", path.display()))?
            {
                None => break,
                Some(parent) => {
                    ensure!(
                        !parent.starts_with(target),
                        "{} depends on the cleanup target",
                        path.display()
                    );
                    next = parent;
                }
            }
        }
    }
    Ok(())
}

pub fn prune(root: &Path, target: &str, execute: bool) -> Result<Value> {
    let _guard = state::storage_lock(root, true)?;
    let root = fs::canonicalize(root)?;
    let (path, owner_dir, directory, name) = resolve(&root, target)?;
    let _owner = state::lock(&owner_dir.join("owner.lock"))?;
    let _runtime = if directory {
        Some(state::lock(&owner_dir.join("runtime.lock"))?)
    } else {
        None
    };
    ensure!(
        real(&owner_dir.join("owner.json"), false)?.len() <= 1_048_576,
        "owner metadata exceeds limit"
    );
    let owner: Session = state::read_json(&owner_dir.join("owner.json"))?;
    ensure!(owner.name == name, "recovery owner mismatch");
    uuid(&owner.id)?;
    if directory {
        ensure!(
            owner.container.is_none(),
            "recovery still records a container; resolve its ownership before cleanup"
        );
        ensure!(
            !owner_dir.join("pid.json").exists(),
            "recovery still records a runtime; resolve it before cleanup"
        );
    }
    let files = inventory(&root)?;
    check_dependencies(&files, &path)?;
    // Media/seed paths are also references, not cleanup candidates.
    for (file, meta) in &files {
        let relative = file.strip_prefix(&root)?;
        let parts: Vec<_> = relative.iter().collect();
        if !file.starts_with(&path)
            && parts.len() == 3
            && (parts[0] == "sessions" || parts[0] == "trash")
            && parts[2] == "owner.json"
        {
            real(file, false)?;
            ensure!(meta.len() <= 1_048_576, "owner metadata exceeds limit");
            let session: Session = state::read_json(file)?;
            for input in [
                &session.config.install_iso,
                &session.config.seed_iso,
                &session.config.answer_disk,
            ]
            .into_iter()
            .flatten()
            {
                let resolved = match fs::canonicalize(input) {
                    Ok(resolved) => resolved,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => input.clone(),
                    Err(error) => return Err(error.into()),
                };
                ensure!(
                    !resolved.starts_with(&path),
                    "a session still references the cleanup target"
                );
            }
        }
    }
    let bytes = usage(&files, &path);
    if execute {
        if directory {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(
        json!({"target":target,"dry_run":!execute,"removed":execute,"usage":bytes,"recoverable_after_removal":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn fixture() -> (tempfile::TempDir, String, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let name = "experiment";
        let slot = format!("{name}-{}", state::id());
        let dir = root.path().join("trash").join(&slot);
        state::private_dir(&dir).unwrap();
        state::write_json(
            &dir.join("owner.json"),
            &Session {
                id: state::id(),
                name: name.into(),
                image: "base".into(),
                engine: "native".into(),
                container: None,
                config: Default::default(),
                viewer_token: String::new(),
            },
        )
        .unwrap();
        fs::write(dir.join("result.txt"), b"retained output").unwrap();
        (root, format!("trash/{slot}"), dir)
    }

    #[test]
    fn cleanup_is_exact_dry_run_and_does_not_follow_share_links() {
        let (root, target, dir) = fixture();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), b"host data").unwrap();
        symlink(outside.path(), dir.join("share-link")).unwrap();
        assert_eq!(
            prune(root.path(), &target, false).unwrap()["removed"],
            false
        );
        assert!(dir.exists());
        assert_eq!(
            report(root.path()).unwrap()["recovery_targets"][0]["target"],
            target
        );
        assert_eq!(prune(root.path(), &target, true).unwrap()["removed"], true);
        assert!(!dir.exists());
        assert!(outside.path().join("keep").exists());
        for bad in [
            "trash",
            "images/base",
            "sessions/experiment",
            "trash/../experiment",
            "recovery/experiment/../system.qcow2",
        ] {
            assert!(prune(root.path(), bad, true).is_err());
        }
    }

    #[test]
    fn live_runtime_and_symlinked_targets_are_refused() {
        let (root, target, dir) = fixture();
        let live = state::lock(&dir.join("runtime.lock")).unwrap();
        assert!(prune(root.path(), &target, true).is_err());
        drop(live);
        fs::rename(&dir, root.path().join("moved")).unwrap();
        symlink(root.path().join("moved"), &dir).unwrap();
        assert!(prune(root.path(), &target, true).is_err());
        assert!(root.path().join("moved/result.txt").exists());
    }

    #[test]
    fn sparse_files_and_hard_links_are_accounted_once() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("media");
        state::private_dir(&dir).unwrap();
        File::create(dir.join("sparse.iso"))
            .unwrap()
            .set_len(1024 * 1024)
            .unwrap();
        fs::hard_link(dir.join("sparse.iso"), dir.join("second.iso")).unwrap();
        let value = report(root.path()).unwrap();
        assert_eq!(value["total"]["logical_bytes"], 1024 * 1024);
        assert_eq!(value["total"]["files"], 1);
        assert_eq!(value["total"]["allocated_bytes"], 0);
    }

    #[test]
    fn cleanup_excludes_concurrent_lifecycle_operations() {
        let (root, target, _) = fixture();
        let guard = state::storage_lock(root.path(), false).unwrap();
        assert!(prune(root.path(), &target, false).is_err());
        drop(guard);
        assert!(prune(root.path(), &target, false).is_ok());
    }

    #[test]
    fn stale_runtime_records_block_cleanup_but_unrelated_json_does_not() {
        let (root, target, dir) = fixture();
        fs::write(dir.join("pid.json"), b"{}").unwrap();
        assert!(prune(root.path(), &target, true).is_err());
        fs::remove_file(dir.join("pid.json")).unwrap();
        state::private_dir(&root.path().join("cache")).unwrap();
        fs::write(root.path().join("cache/owner.json"), b"not owner state").unwrap();
        assert!(prune(root.path(), &target, false).is_ok());
    }

    #[test]
    fn backing_dependencies_block_cleanup() {
        let (root, target, dir) = fixture();
        let disk = dir.join("system.qcow2");
        fs::write(&disk, b"held target").unwrap();
        let dependent = root.path().join("images/dependent");
        state::private_dir(&dependent).unwrap();
        let name = disk.as_os_str().as_encoded_bytes();
        let mut header = vec![0u8; 104];
        header[..4].copy_from_slice(b"QFI\xfb");
        header[4..8].copy_from_slice(&3u32.to_be_bytes());
        header[8..16].copy_from_slice(&104u64.to_be_bytes());
        header[16..20].copy_from_slice(&(name.len() as u32).to_be_bytes());
        header.extend_from_slice(name);
        fs::write(dependent.join("base.qcow2"), header).unwrap();
        assert!(
            prune(root.path(), &target, true)
                .unwrap_err()
                .to_string()
                .contains("depends")
        );
        assert!(disk.exists());
    }
}
