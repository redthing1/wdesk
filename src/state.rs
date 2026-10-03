use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub fn id() -> String {
    Uuid::new_v4().to_string()
}
pub fn secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 48
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "name must contain 1..48 ASCII letters, digits, - or _"
    );
    Ok(())
}

pub fn root() -> PathBuf {
    std::env::var_os("WDESK_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").expect("HOME required"))
                        .join(".local/share")
                })
                .join("wdesk")
        })
}

pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    private_dir(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    temp.write_all(&serde_json::to_vec_pretty(value)?)?;
    temp.as_file().sync_all()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o600))?;
    temp.persist(path)?;
    Ok(())
}

pub fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        .context("invalid descriptor")
}

pub fn lock(path: &Path) -> Result<fs::File> {
    private_dir(path.parent().context("missing lock parent")?)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    fs2::FileExt::try_lock_exclusive(&file).context("another operation owns this session/image")?;
    Ok(file)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub protocol: u32,
    pub endpoint: String,
    pub token: String,
    pub epoch: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct VmConfig {
    pub memory_mb: u32,
    pub cpus: u32,
    pub offline: bool,
    pub install_iso: Option<PathBuf>,
    pub seed_iso: Option<PathBuf>,
    #[serde(default)]
    pub answer_disk: Option<PathBuf>,
    pub tcg: bool,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            memory_mb: 4096,
            cpus: 2,
            offline: false,
            install_iso: None,
            seed_iso: None,
            answer_disk: None,
            tcg: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub image: String,
    pub engine: String,
    pub container: Option<String>,
    pub config: VmConfig,
    pub viewer_token: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub protocol: u32,
    pub name: String,
    pub image_id: String,
    pub media_sha256: Option<String>,
    pub guest_build: serde_json::Value,
    pub profile: String,
    pub generalized: bool,
    pub helper_sha256: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_cannot_escape_names() {
        for name in ["../x", "", "a/b", ".", "a,b", "a b"] {
            assert!(validate_name(name).is_err());
        }
        validate_name("test-01").unwrap();
    }
    #[test]
    fn descriptors_are_private_and_only_data_plane() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("client.json");
        write_json(
            &p,
            &Descriptor {
                protocol: 1,
                endpoint: "http://localhost:1".into(),
                token: secret(),
                epoch: id(),
            },
        )
        .unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _: Descriptor = read_json(&p).unwrap();
        let bytes = fs::read_to_string(p).unwrap();
        assert!(!bytes.contains("engine"));
    }
}
