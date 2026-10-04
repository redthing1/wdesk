use crate::{
    client::Client,
    state::{self, Descriptor, Manifest, Session, VmConfig},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

pub const RUNNER_IMAGE: &str = "localhost/wdesk:0.1.0";
const LABEL: &str = "io.wdesk.session";

fn session_dir(name: &str) -> Result<PathBuf> {
    state::validate_name(name)?;
    Ok(state::root().join("sessions").join(name))
}
fn image_dir(name: &str) -> Result<PathBuf> {
    state::validate_name(name)?;
    Ok(state::root().join("images").join(name))
}
async fn command(program: &str, args: &[String]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .with_context(|| format!("running {program}"))?;
    ensure!(
        output.status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}
fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|x| (*x).into()).collect()
}
fn load(name: &str) -> Result<Session> {
    state::read_json(&session_dir(name)?.join("owner.json"))
}

pub async fn doctor() -> Result<Value> {
    let mut checks = serde_json::Map::new();
    for (name, args) in [
        ("qemu-system-x86_64", vec!["--version"]),
        ("qemu-img", vec!["--version"]),
        ("xorriso", vec!["-version"]),
        ("mformat", vec!["-V"]),
        ("mcopy", vec!["-V"]),
        (
            "podman",
            vec!["info", "--format", "{{.Host.Security.Rootless}}"],
        ),
        ("docker", vec!["info", "--format", "{{.ServerVersion}}"]),
    ] {
        let result = command(name, &strings(&args)).await;
        checks.insert(
            name.into(),
            match result {
                Ok(text) => json!({"available":true,"detail":text.lines().next().unwrap_or("")}),
                Err(error) => json!({"available":false,"detail":error.to_string()}),
            },
        );
    }
    checks.insert("kvm".into(),json!({"accessible":fs::OpenOptions::new().read(true).write(true).open("/dev/kvm").is_ok()}));
    Ok(json!({"home":state::root(),"checks":checks}))
}

pub fn images() -> Result<Value> {
    let path = state::root().join("images");
    let mut images = vec![];
    if path.exists() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let manifest = entry.path().join("manifest.json");
            if manifest.is_file() {
                images.push(state::read_json::<Value>(&manifest)?);
            }
        }
    }
    Ok(json!(images))
}

pub async fn build(engine: &str, source: &Path, shares: bool) -> Result<()> {
    ensure!(["podman", "docker"].contains(&engine), "invalid engine");
    let source = fs::canonicalize(source).context("runner source checkout not found")?;
    for path in ["Cargo.toml", "Cargo.lock", "container/Containerfile"] {
        ensure!(
            source.join(path).is_file(),
            "missing {path}; run from a wdesk checkout or pass --source PATH"
        );
    }
    let status = Command::new(engine)
        .args(["build", "-f"])
        .arg(source.join("container/Containerfile"))
        .args([
            "--build-arg",
            if shares {
                "WDESK_SHARES=1"
            } else {
                "WDESK_SHARES=0"
            },
        ])
        .args(["-t", RUNNER_IMAGE])
        .arg(&source)
        .status()
        .await?;
    ensure!(status.success(), "runner image build failed");
    Ok(())
}

pub async fn fetch(url: &str, expected: &str, output: &Path) -> Result<Value> {
    fetch_limited(url, expected, output, 16 * 1024 * 1024 * 1024).await
}

pub(crate) async fn fetch_limited(
    url: &str,
    expected: &str,
    output: &Path,
    limit: u64,
) -> Result<Value> {
    ensure!(
        !output.exists(),
        "output already exists; choose another path"
    );
    ensure!(
        expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()),
        "required SHA-256 must be 64 hex digits"
    );
    ensure!(
        reqwest::Url::parse(url)?.scheme() == "https",
        "media fetch requires HTTPS"
    );
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    let mut file = tokio::fs::File::from_std(temp.reopen()?);
    let mut response = reqwest::Client::builder()
        .timeout(Duration::from_secs(3600))
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    let mut hash = Sha256::new();
    let mut size = 0u64;
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = response.chunk().await? {
        size += chunk.len() as u64;
        ensure!(size <= limit, "download exceeds declared byte limit");
        hash.update(&chunk);
        file.write_all(&chunk).await?;
    }
    let actual = hex::encode(hash.finalize());
    ensure!(
        actual.eq_ignore_ascii_case(expected),
        "media SHA-256 mismatch: {actual}"
    );
    file.sync_all().await?;
    drop(file);
    temp.persist_noclobber(output)?;
    Ok(json!({"path":output,"sha256":actual,"bytes":size,"source":url}))
}

pub async fn open(name: &str, image: &str, engine: &str, config: VmConfig) -> Result<()> {
    let _storage = state::storage_lock(&state::root(), false)?;
    let dir = session_dir(name)?;
    state::private_dir(&dir)?;
    let _lock = state::lock(&dir.join("owner.lock"))?;
    let owner_path = dir.join("owner.json");
    let session = if owner_path.exists() {
        let session: Session = state::read_json(&owner_path)?;
        ensure!(session.name == name, "session identity mismatch");
        session
    } else {
        ensure!(
            ["native", "podman", "docker"].contains(&engine),
            "unsupported engine"
        );
        let image_path = image_dir(image)?;
        let _: Manifest = state::read_json(&image_path.join("manifest.json"))
            .context("image not prepared; use wdesk image install/import")?;
        create_overlay(&image_path.join("base.qcow2"), &dir.join("system.qcow2")).await?;
        let session = Session {
            id: state::id(),
            name: name.into(),
            image: image.into(),
            engine: engine.into(),
            container: None,
            config,
            viewer_token: String::new(),
        };
        state::write_json(&dir.join("vm.json"), &session.config)?;
        state::write_json(&owner_path, &session)?;
        session
    };
    start(&dir, session).await
}

async fn create_overlay(base: &Path, target: &Path) -> Result<()> {
    ensure!(!target.exists(), "overlay already exists");
    command(
        "qemu-img",
        &[
            "create".into(),
            "-f".into(),
            "qcow2".into(),
            "-F".into(),
            "qcow2".into(),
            "-b".into(),
            fs::canonicalize(base)?.to_string_lossy().into_owned(),
            target.to_string_lossy().into_owned(),
        ],
    )
    .await?;
    Ok(())
}

async fn start(dir: &Path, mut session: Session) -> Result<()> {
    crate::shares::validate(&session.config.shares)?;
    // Owner state is authoritative; runtime configuration is regenerated so a
    // failed multi-file update cannot silently restore an old share grant.
    state::write_json(&dir.join("vm.json"), &session.config)?;
    if session.engine == "native" {
        if native_running(dir, &session.id)? {
            refresh_descriptor(dir, None, &mut session).await?;
            return Ok(());
        }
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("runtime.log"))?;
        let child = Command::new("setsid")
            // A package upgrade may unlink this CLI while an installation runs.
            // Execute its live inode, not the now-stale installed pathname.
            .arg(format!("/proc/{}/exe", std::process::id()))
            .args(["serve", "--state-dir"])
            .arg(dir)
            .args(["--owner", &session.id])
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        let pid = child.id().context("native runtime PID unavailable")?;
        let start = proc_start(pid).context("native runtime exited during startup")?;
        state::write_json(&dir.join("pid.json"), &json!({"pid":pid,"start":start}))?;
        drop(child);
        refresh_descriptor(dir, None, &mut session).await?;
    } else {
        let container = session
            .container
            .clone()
            .unwrap_or_else(|| format!("wdesk-{}", session.id));
        if container_exists(&session.engine, &container).await? {
            verify_container(&session.engine, &container, &session.id).await?;
            command(&session.engine, &["start".into(), container.clone()]).await?;
        } else {
            // Base image is mounted at the identical absolute path used by qcow2.
            let images = fs::canonicalize(state::root().join("images"))?;
            let gid = command("stat", &strings(&["-c", "%g", "/dev/kvm"]))
                .await?
                .trim()
                .to_owned();
            let uid = command("id", &strings(&["-u"])).await?.trim().to_owned();
            let group = command("id", &strings(&["-g"])).await?.trim().to_owned();
            let mut args = vec![
                "run".into(),
                "-d".into(),
                "--name".into(),
                container.clone(),
                "--label".into(),
                format!("{LABEL}={}", session.id),
                "--device".into(),
                "/dev/kvm".into(),
                "--cap-drop=ALL".into(),
                "--security-opt=no-new-privileges".into(),
                "--read-only".into(),
                "--tmpfs".into(),
                "/tmp:rw,nosuid,nodev,size=256m".into(),
                "--pids-limit=256".into(),
                "--user".into(),
                format!("{uid}:{group}"),
                "-v".into(),
                format!("{}:/state:rw", dir.display()),
                "-v".into(),
                format!("{}:{}:ro", images.display(), images.display()),
                "-p".into(),
                "127.0.0.1::9841".into(),
            ];
            if session.engine == "podman" {
                args.extend(["--userns=keep-id".into(), "--group-add=keep-groups".into()]);
            } else {
                args.extend(["--group-add".into(), gid]);
            }
            for path in [
                session.config.install_iso.as_deref(),
                session.config.seed_iso.as_deref(),
                session.config.answer_disk.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                args.extend([
                    "-v".into(),
                    format!("{}:{}:ro", path.display(), path.display()),
                ]);
            }
            for share in &session.config.shares {
                args.extend([
                    "--mount".into(),
                    format!(
                        "type=bind,src={},dst={}{}",
                        share.path.display(),
                        share.path.display(),
                        if share.read_only { ",readonly" } else { "" }
                    ),
                ]);
            }
            args.extend([
                RUNNER_IMAGE.into(),
                "serve".into(),
                "--state-dir".into(),
                "/state".into(),
                "--bind".into(),
                "0.0.0.0:9841".into(),
                "--owner".into(),
                session.id.clone(),
            ]);
            command(&session.engine, &args).await?;
        }
        session.container = Some(container.clone());
        state::write_json(&dir.join("owner.json"), &session)?;
        refresh_descriptor(dir, Some(&container), &mut session).await?;
    }
    state::write_json(&dir.join("owner.json"), &session)?;
    Ok(())
}

async fn refresh_descriptor(
    dir: &Path,
    container: Option<&str>,
    session: &mut Session,
) -> Result<()> {
    let mut ready = None;
    for _ in 0..200 {
        if let Ok(descriptor) = state::read_json::<Descriptor>(&dir.join("runtime-client.json")) {
            let mut descriptor = descriptor;
            if let Some(container) = container {
                let output = command(
                    &session.engine,
                    &["port".into(), container.into(), "9841/tcp".into()],
                )
                .await?;
                let port = output
                    .trim()
                    .split(':')
                    .next_back()
                    .context("no published runtime port")?;
                let port: u16 = port.parse()?;
                descriptor.endpoint = format!("http://127.0.0.1:{port}");
            }
            if Client::new(descriptor.clone())?
                .get("/v1/health")
                .await
                .is_ok()
            {
                ready = Some(descriptor);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let descriptor =
        ready.context("runtime did not start; inspect runtime.log/qemu.log or engine logs")?;
    let viewer: Value = state::read_json(&dir.join("runtime-viewer.json"))?;
    session.viewer_token = viewer["token"]
        .as_str()
        .context("viewer credential missing")?
        .into();
    state::write_json(&dir.join("client.json"), &descriptor)?;
    Ok(())
}

async fn container_exists(engine: &str, container: &str) -> Result<bool> {
    let output = Command::new(engine)
        .args(["container", "inspect", container])
        .output()
        .await?;
    Ok(output.status.success())
}
async fn verify_container(engine: &str, container: &str, id: &str) -> Result<()> {
    let output = command(
        engine,
        &[
            "container".into(),
            "inspect".into(),
            container.into(),
            "--format".into(),
            format!("{{{{ index .Config.Labels \"{LABEL}\" }}}}"),
        ],
    )
    .await?;
    ensure!(
        output.trim() == id,
        "refusing to manage a container without matching ownership label"
    );
    Ok(())
}

fn proc_start(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return None;
    }
    fields.get(19).map(|s| (*s).to_owned())
}
fn native_running(dir: &Path, id: &str) -> Result<bool> {
    if !dir.join("pid.json").exists() {
        return Ok(false);
    }
    let pid: Value = state::read_json(&dir.join("pid.json"))?;
    let number = pid["pid"].as_u64().context("invalid runtime PID")? as u32;
    if proc_start(number).as_deref() != pid["start"].as_str() {
        return Ok(false);
    }
    let Ok(cmd) = fs::read(format!("/proc/{number}/cmdline")) else {
        return Ok(false);
    };
    let args: Vec<&[u8]> = cmd.split(|b| *b == 0).filter(|a| !a.is_empty()).collect();
    if args.is_empty() {
        return Ok(false);
    }
    ensure!(
        args.windows(2)
            .any(|pair| pair[0] == b"--owner" && pair[1] == id.as_bytes()),
        "runtime PID ownership mismatch"
    );
    Ok(true)
}

async fn stop_locked(dir: &Path, session: &Session) -> Result<()> {
    if session.engine == "native" {
        if native_running(dir, &session.id)? {
            let pid: Value = state::read_json(&dir.join("pid.json"))?;
            command("kill", &["-TERM".into(), pid["pid"].to_string()]).await?;
            for _ in 0..250 {
                if !native_running(dir, &session.id)? {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            ensure!(
                !native_running(dir, &session.id)?,
                "runtime has not stopped; inspect logs before retrying"
            );
        }
    } else if let Some(container) = &session.container
        && container_exists(&session.engine, container).await?
    {
        verify_container(&session.engine, container, &session.id).await?;
        command(
            &session.engine,
            &[
                "stop".into(),
                "--time".into(),
                "25".into(),
                container.clone(),
            ],
        )
        .await?;
    }
    for path in [
        "client.json",
        "runtime-client.json",
        "runtime-viewer.json",
        "pid.json",
    ] {
        let _ = fs::remove_file(dir.join(path));
    }
    Ok(())
}
pub async fn stop(name: &str) -> Result<()> {
    let dir = session_dir(name)?;
    let _lock = state::lock(&dir.join("owner.lock"))?;
    stop_locked(&dir, &load(name)?).await
}

pub fn list_shares(name: &str) -> Result<Value> {
    Ok(json!(load(name)?.config.shares))
}

pub async fn change_share(
    name: &str,
    share_name: &str,
    path: Option<&Path>,
    write: bool,
) -> Result<Value> {
    let dir = session_dir(name)?;
    let _lock = state::lock(&dir.join("owner.lock"))?;
    let mut session = load(name)?;
    state::validate_name(share_name)?;
    // Revocation must never leave a running server with a stale grant.
    ensure!(
        !dir.join("client.json").exists(),
        "stop this session before changing share grants"
    );
    if session.engine == "native" {
        ensure!(
            !native_running(&dir, &session.id)?,
            "stop this session first"
        );
    } else if let Some(container) = &session.container
        && container_exists(&session.engine, container).await?
    {
        verify_container(&session.engine, container, &session.id).await?;
        let running = command(
            &session.engine,
            &[
                "inspect".into(),
                container.clone(),
                "--format".into(),
                "{{.State.Running}}".into(),
            ],
        )
        .await?;
        ensure!(running.trim() != "true", "stop this session first");
    }
    if let Some(path) = path {
        ensure!(
            !session
                .config
                .shares
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(share_name)),
            "share name already granted; remove it first"
        );
        let share = crate::shares::grant(share_name, path, !write)?;
        let root = fs::canonicalize(state::root())?;
        ensure!(
            !share.path.starts_with(&root) && !root.starts_with(&share.path),
            "share must not overlap private wdesk state"
        );
        session.config.shares.push(share);
        crate::shares::validate(&session.config.shares)?;
    } else {
        let before = session.config.shares.len();
        session
            .config
            .shares
            .retain(|s| !s.name.eq_ignore_ascii_case(share_name));
        ensure!(session.config.shares.len() != before, "share not granted");
    }
    remove_container(&session).await?;
    session.container = None;
    state::write_json(&dir.join("owner.json"), &session)?;
    Ok(
        json!({"grants":crate::shares::public(&session.config.shares),"next":"open resumes with these grants","host_writes_rollback":false}),
    )
}

pub async fn delete(name: &str) -> Result<()> {
    let _storage = state::storage_lock(&state::root(), false)?;
    let dir = session_dir(name)?;
    let _lock = state::lock(&dir.join("owner.lock"))?;
    let mut session = load(name)?;
    ensure!(session.name == name, "session name mismatch");
    stop_locked(&dir, &session).await?;
    remove_container(&session).await?;
    session.container = None;
    state::write_json(&dir.join("owner.json"), &session)?;
    // Move to a private trash area; never recursively delete a caller-supplied path.
    let trash = state::root().join("trash");
    state::private_dir(&trash)?;
    fs::rename(&dir, trash.join(format!("{name}-{}", state::id())))?;
    Ok(())
}
async fn remove_container(session: &Session) -> Result<()> {
    if let Some(container) = &session.container
        && container_exists(&session.engine, container).await?
    {
        verify_container(&session.engine, container, &session.id).await?;
        command(&session.engine, &["rm".into(), container.clone()]).await?;
    }
    Ok(())
}
pub async fn reset(name: &str) -> Result<()> {
    let _storage = state::storage_lock(&state::root(), false)?;
    let dir = session_dir(name)?;
    let _lock = state::lock(&dir.join("owner.lock"))?;
    let mut session = load(name)?;
    ensure!(
        session.config.install_iso.is_none(),
        "seal the installation before resetting it"
    );
    stop_locked(&dir, &session).await?;
    remove_container(&session).await?;
    let old = dir.join(format!("system-before-reset-{}.qcow2", state::id()));
    fs::rename(dir.join("system.qcow2"), &old)?;
    if let Err(e) = create_overlay(
        &image_dir(&session.image)?.join("base.qcow2"),
        &dir.join("system.qcow2"),
    )
    .await
    {
        fs::rename(&old, dir.join("system.qcow2"))?;
        return Err(e);
    }
    session.id = state::id();
    session.container = None;
    session.viewer_token.clear();
    state::write_json(&dir.join("owner.json"), &session)?;
    // Recoverable old overlay retained until the user removes it from local state.
    start(&dir, session).await
}

pub fn viewer_url(name: &str) -> Result<String> {
    let session = load(name)?;
    let client: Descriptor = state::read_json(&session_dir(name)?.join("client.json"))?;
    Ok(format!(
        "{}/viewer#token={}",
        client.endpoint, session.viewer_token
    ))
}

pub async fn wait(name: &str, seconds: u64) -> Result<Value> {
    let client = Client::discover(name, None)?;
    let needs_shares = load(name).is_ok_and(|s| !s.config.shares.is_empty());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let screen = tempfile::NamedTempFile::new()?;
    let mut last = String::new();
    loop {
        if let Ok(health) = client.get("/v1/health").await {
            let state = health["state"].as_str().unwrap_or("unknown");
            if state != last {
                eprintln!("wdesk {name}: {state}");
                last = state.into();
            }
            if ["helper_ready", "automation_ready"].contains(&state)
                && client.screenshot(screen.path()).await.is_ok()
            {
                if needs_shares {
                    ensure!(
                        health["guest"]["features"]["host_shares"] == 1,
                        "this image needs a current host-share-capable helper; session kept for inspection"
                    );
                    let caps = client.get("/v1/capabilities").await?;
                    if caps["shares"]["attached"] != true {
                        if last != "attaching_shares" {
                            eprintln!("wdesk {name}: attaching_shares");
                            last = "attaching_shares".into();
                        }
                        ensure!(
                            tokio::time::Instant::now() < deadline,
                            "share attachment deadline; inspect capabilities.shares.error"
                        );
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
                return client.get("/v1/health").await;
            }
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "readiness deadline exceeded ({last}); session kept running, inspect wdesk --session {name} view and logs"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

pub fn sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 1024 * 1024];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}

pub async fn import_image(
    disk: &Path,
    name: &str,
    profile: &str,
    media: Option<String>,
    guest: Value,
    compress: bool,
) -> Result<Value> {
    let _storage = state::storage_lock(&state::root(), false)?;
    let dir = image_dir(name)?;
    state::private_dir(dir.parent().context("image parent")?)?;
    let _lock = state::lock(&dir.with_extension("lock"))?;
    ensure!(
        !dir.exists(),
        "image exists; use a new name to preserve sealed bases"
    );
    let temp = tempfile::Builder::new()
        .prefix("image-")
        .tempdir_in(dir.parent().unwrap())?;
    let mut args = strings(&["convert", "-O", "qcow2"]);
    if compress {
        args.extend(strings(&["-c", "-o", "compression_type=zstd"]));
    }
    args.push(disk.to_string_lossy().into_owned());
    args.push(
        temp.path()
            .join("base.qcow2")
            .to_string_lossy()
            .into_owned(),
    );
    command("qemu-img", &args).await?;
    let helper_sha256 = guest["helper_sha256"]
        .as_str()
        .unwrap_or("unverified")
        .to_owned();
    let manifest = Manifest {
        protocol: 1,
        name: name.into(),
        image_id: state::id(),
        media_sha256: media,
        guest_build: guest,
        profile: profile.into(),
        generalized: false,
        helper_sha256,
    };
    state::write_json(&temp.path().join("manifest.json"), &manifest)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        temp.path().join("base.qcow2"),
        fs::Permissions::from_mode(0o444),
    )?;
    let temp_path = temp.keep();
    fs::rename(temp_path, &dir)?;
    Ok(serde_json::to_value(manifest)?)
}

pub async fn seal(
    session_name: &str,
    name: &str,
    profile: &str,
    media: Option<&Path>,
    compress: bool,
) -> Result<Value> {
    let _storage = state::storage_lock(&state::root(), false)?;
    let dir = session_dir(session_name)?;
    let _lock = state::lock(&dir.join("owner.lock"))?;
    let session = load(session_name)?;
    ensure!(
        session.config.shares.is_empty(),
        "remove live share grants before sealing an independent image"
    );
    let guest = if let Ok(client) = Client::discover(session_name, None) {
        client.guest("health", json!({})).await.unwrap_or(json!({}))
    } else {
        json!({})
    };
    stop_locked(&dir, &session).await?;
    let media_hash = if let Some(media) = media.or(session.config.install_iso.as_deref()) {
        Some(sha256(media)?)
    } else {
        let path = image_dir(&session.image)?.join("manifest.json");
        if path.exists() {
            state::read_json::<Manifest>(&path)?.media_sha256
        } else {
            None
        }
    };
    import_image(
        &dir.join("system.qcow2"),
        name,
        profile,
        media_hash,
        guest,
        compress,
    )
    .await
}

pub struct InstallOptions<'a> {
    pub profile: &'a str,
    pub index: u32,
    pub disk_gb: u32,
    pub engine: &'a str,
    pub install_key: &'a str,
    pub compact_os: bool,
}

pub fn validate_compact_profile(profile: &str, compact_os: bool) -> Result<()> {
    ensure!(
        !compact_os || profile == "core",
        "--compact-os currently requires Core media/profile; use --compress for regular media"
    );
    Ok(())
}

fn answer_file(
    password: &str,
    index: u32,
    install_key: &str,
    profile: &str,
    compact_os: bool,
) -> String {
    include_str!("../guest/autounattend.xml")
        .replace("@PASSWORD@", password)
        .replace("@INDEX@", &index.to_string())
        .replace("@INSTALL_KEY@", install_key)
        .replace("@PROFILE@", profile)
        .replace(
            "@COMPACT@",
            if compact_os {
                "<Compact>true</Compact>"
            } else {
                ""
            },
        )
}

pub async fn install(iso: &Path, name: &str, options: InstallOptions<'_>) -> Result<()> {
    let _storage = state::storage_lock(&state::root(), false)?;
    let InstallOptions {
        profile,
        index,
        disk_gb,
        engine,
        install_key,
        compact_os,
    } = options;
    validate_compact_profile(profile, compact_os)?;
    state::validate_name(name)?;
    ensure!((1..=100).contains(&index), "invalid WIM index");
    ensure!(
        install_key.len() == 29
            && install_key
                .bytes()
                .enumerate()
                .all(|(i, b)| if [5, 11, 17, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_alphanumeric()
                }),
        "invalid edition installation key"
    );
    ensure!((24..=512).contains(&disk_gb), "disk must be 24..512 GiB");
    ensure!(!image_dir(name)?.exists(), "image already exists");
    let iso = fs::canonicalize(iso)?;
    ensure!(iso.is_file(), "ISO must be a file");
    let session_name = format!("build-{name}");
    let dir = session_dir(&session_name)?;
    state::private_dir(&dir)?;
    let _lock = state::lock(&dir.join("owner.lock"))?;
    ensure!(
        !dir.join("owner.json").exists(),
        "installation session already exists; resume it with open/wait/seal"
    );
    let seed_source = dir.join("seed");
    state::private_dir(&seed_source)?;
    fs::write(
        seed_source.join("agent.ps1"),
        include_bytes!("../guest/agent.ps1"),
    )?;
    fs::write(
        seed_source.join("native.cs"),
        include_bytes!("../guest/native.cs"),
    )?;
    fs::write(
        seed_source.join("a11y.ps1"),
        include_bytes!("../guest/a11y.ps1"),
    )?;
    fs::write(
        seed_source.join("files.cs"),
        include_bytes!("../guest/files.cs"),
    )?;
    fs::write(
        seed_source.join("graphics.cs"),
        include_bytes!("../guest/graphics.cs"),
    )?;
    fs::write(
        seed_source.join("shares.cs"),
        include_bytes!("../guest/shares.cs"),
    )?;
    fs::write(
        seed_source.join("setup.ps1"),
        include_bytes!("../guest/setup.ps1"),
    )?;
    let password = state::secret();
    let xml = answer_file(&password, index, install_key, profile, compact_os);
    fs::write(seed_source.join("autounattend.xml"), xml)?;
    let seed = dir.join("seed.iso");
    command(
        "xorriso",
        &[
            "-as".into(),
            "mkisofs".into(),
            "-J".into(),
            "-r".into(),
            "-V".into(),
            "WDESK".into(),
            "-o".into(),
            seed.to_string_lossy().into_owned(),
            seed_source.to_string_lossy().into_owned(),
        ],
    )
    .await?;
    command(
        "qemu-img",
        &[
            "create".into(),
            "-f".into(),
            "qcow2".into(),
            dir.join("system.qcow2").to_string_lossy().into_owned(),
            format!("{disk_gb}G"),
        ],
    )
    .await?;
    let answer = dir.join("unattend.img");
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&answer)?
        .set_len(1440 * 1024)?;
    command(
        "mformat",
        &[
            "-i".into(),
            answer.to_string_lossy().into_owned(),
            "-f".into(),
            "1440".into(),
            "::".into(),
        ],
    )
    .await?;
    command(
        "mcopy",
        &[
            "-i".into(),
            answer.to_string_lossy().into_owned(),
            seed_source
                .join("autounattend.xml")
                .to_string_lossy()
                .into_owned(),
            "::autounattend.xml".into(),
        ],
    )
    .await?;
    let config = VmConfig {
        install_iso: Some(iso),
        seed_iso: Some(seed),
        answer_disk: Some(answer),
        // Installation is self-contained; avoid OOBE network/update detours.
        offline: true,
        ..Default::default()
    };
    let session = Session {
        id: state::id(),
        name: session_name,
        image: name.into(),
        engine: engine.into(),
        container: None,
        config,
        viewer_token: String::new(),
    };
    state::write_json(&dir.join("vm.json"), &session.config)?;
    state::write_json(&dir.join("owner.json"), &session)?;
    let session_name = session.name.clone();
    start(&dir, session).await?;
    // Windows installation media can require a key to boot its CD.
    let client = Client::discover(&session_name, None)?;
    let screen = tempfile::NamedTempFile::new()?;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let observation = client.screenshot(screen.path()).await?;
        if observation["geometry"]["width"].as_u64().unwrap_or(0) >= 800 {
            break;
        }
        client
            .action(
                vec![crate::protocol::Action::Key {
                    keys: vec!["ENTER".into()],
                }],
                None,
            )
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod installation_tests {
    use super::{answer_file, build, validate_compact_profile};

    #[tokio::test]
    async fn runner_build_requires_a_source_checkout_before_invoking_engine() {
        let source = tempfile::tempdir().unwrap();
        let error = build("docker", source.path(), false).await.unwrap_err();
        assert!(error.to_string().contains("missing Cargo.toml"));
        let error = build("unknown", source.path(), false).await.unwrap_err();
        assert!(error.to_string().contains("invalid engine"));
    }

    #[test]
    fn compact_installation_is_limited_to_validated_core_media() {
        for profile in ["reference", "lite", "core"] {
            assert!(validate_compact_profile(profile, false).is_ok());
            assert_eq!(
                validate_compact_profile(profile, true).is_ok(),
                profile == "core"
            );
        }
    }

    #[test]
    fn compact_os_is_an_explicit_windows_setup_option() {
        for (enabled, setting) in [
            (false, "<OSImage><InstallFrom>"),
            (true, "<OSImage><Compact>true</Compact><InstallFrom>"),
        ] {
            let xml = answer_file("test-password", 3, "test-key", "core", enabled);
            assert!(xml.contains(setting));
            assert!(xml.contains("<Value>3</Value>"));
            assert!(xml.contains("-Profile 'core'"));
            assert!(!xml.contains("@COMPACT@"));
            assert!(!xml.contains("@PASSWORD@"));
            assert_eq!(xml.matches("<Compact>").count(), usize::from(enabled));
        }
    }
}
