//! Optional per-VM SMB, never QEMU's anonymous SMB convenience backend.
use crate::state::{self, Share};
use anyhow::{Context, Result, ensure};
use md4::{Digest, Md4};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    net::{TcpStream, UnixListener, UnixStream},
    process::{Child, Command},
    time::{Duration, sleep},
};

pub const ADDRESS: &str = "10.0.2.102";
const USER: &str = "wdesk";

pub async fn relay(socket: &Path) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let stream = UnixStream::connect(socket).await?;
    let (mut read, mut write) = stream.into_split();
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    tokio::select! {
        result=async {
            tokio::io::copy(&mut input,&mut write).await?;
            write.shutdown().await?;
            Ok::<(),std::io::Error>(())
        } => result?,
        result=tokio::io::copy(&mut read,&mut output) => {result?;output.flush().await?;}
    }
    Ok(())
}

fn safe_value(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && !value
                .chars()
                .any(|c| c.is_control() || ['%', '#', ';', '"', '\\', '[', ']', ','].contains(&c))
            && value.trim() == value,
        "share paths cannot contain Samba configuration metacharacters"
    );
    Ok(())
}

pub fn grant(name: &str, path: &Path, read_only: bool) -> Result<Share> {
    state::validate_name(name)?;
    ensure!(
        !["global", "homes", "printers", "ipc", "print"]
            .contains(&name.to_ascii_lowercase().as_str()),
        "reserved share name"
    );
    let path = fs::canonicalize(path).context("share directory must already exist")?;
    safe_value(path.to_str().context("share path must be UTF-8")?)?;
    ensure!(
        path != Path::new("/"),
        "grant a specific directory, not the filesystem root"
    );
    let meta = fs::symlink_metadata(&path)?;
    ensure!(meta.is_dir(), "share must be a directory");
    Ok(Share {
        name: name.into(),
        path,
        read_only,
        device: meta.dev(),
        inode: meta.ino(),
    })
}

pub fn validate(shares: &[Share]) -> Result<()> {
    ensure!(shares.len() <= 8, "at most eight explicit share grants");
    for (i, share) in shares.iter().enumerate() {
        let now = grant(&share.name, &share.path, share.read_only)?;
        ensure!(
            now.path == share.path && now.device == share.device && now.inode == share.inode,
            "share directory identity changed; revoke and explicitly grant it again"
        );
        ensure!(
            !shares[..i]
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(&share.name)),
            "duplicate share name"
        );
    }
    Ok(())
}

pub fn public(shares: &[Share]) -> Value {
    json!(
        shares
            .iter()
            .map(
                |s| json!({"name":s.name,"path":format!(r"\\{ADDRESS}\{}",s.name),
        "read_only":s.read_only})
            )
            .collect::<Vec<_>>()
    )
}

fn config(shares: &[Share], run: &Path, port: u16) -> Result<String> {
    validate(shares)?;
    let run = run
        .to_str()
        .context("share runtime directory must be UTF-8")?;
    safe_value(run)?;
    let mut out = format!(
        "[global]\nserver role = standalone server\nworkgroup = WORKGROUP\nnetbios name = WDESK\nsecurity = user\nmap to guest = Never\nguest account = nobody\npassdb backend = smbpasswd:{run}/passwords\nprivate dir = {run}/private\nstate directory = {run}/state\ncache directory = {run}/state\nlock directory = {run}/lock\npid directory = {run}/lock\nncalrpc dir = {run}/lock/ncalrpc\nlog file = {run}/smbd.log\nmax log size = 128\ninterfaces = 127.0.0.1\nbind interfaces only = yes\nsmb ports = {port}\nserver min protocol = SMB2_10\nserver signing = mandatory\nserver multi channel support = no\nload printers = no\ndisable spoolss = yes\nprinting = bsd\nprintcap name = /dev/null\ndns proxy = no\nunix extensions = no\nusershare max shares = 0\nwide links = no\nfollow symlinks = no\nallow insecure wide links = no\nmax connections = 8\nntlm auth = ntlmv2-only\nrpc_server:epmapper = disabled\nrpc_server:spoolss = disabled\nrpc_server:winreg = disabled\nrpc_daemon:spoolssd = disabled\nrpc_daemon:mdssd = disabled\nrpc_server:mdssvc = disabled\n"
    );
    // Host-local writers cannot invalidate SMB client leases. Live grants must
    // read through the server rather than retain stale guest-side file data.
    out.push_str("oplocks = no\nlevel2 oplocks = no\nsmb2 leases = no\n");
    for share in shares {
        out.push_str(&format!("\n[{}]\npath = {}\nvalid users = {USER}\nguest ok = no\nbrowseable = no\nread only = {}\n", share.name, share.path.display(), if share.read_only {"yes"} else {"no"}));
    }
    Ok(out)
}

fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn nt_hash(password: &str) -> String {
    let mut hash = Md4::new();
    for unit in password.encode_utf16() {
        hash.update(unit.to_le_bytes());
    }
    hex::encode_upper(hash.finalize())
}

fn log_tail(run: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = fs::File::open(run.join("smbd.log")) else {
        return "no Samba log available".into();
    };
    let Ok(meta) = file.metadata() else {
        return "Samba log unavailable".into();
    };
    let _ = file.seek(SeekFrom::Start(meta.len().saturating_sub(8192)));
    let mut bytes = Vec::new();
    let _ = file.take(8192).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn wrapper() -> Result<PathBuf> {
    let paths = std::env::var_os("WDESK_NSS_WRAPPER")
        .map(PathBuf::from)
        .into_iter()
        .chain([
            PathBuf::from("/usr/lib/x86_64-linux-gnu/libnss_wrapper.so"),
            PathBuf::from("/usr/lib64/libnss_wrapper.so"),
            PathBuf::from("/usr/lib/libnss_wrapper.so"),
        ]);
    paths
        .into_iter()
        .find(|p| p.is_file())
        .context("optional shares require libnss-wrapper (or WDESK_NSS_WRAPPER)")
}

pub struct Server {
    child: Child,
    group: u32,
    run: PathBuf,
    proxy: tokio::task::JoinHandle<()>,
    password: String,
    grants: Vec<Share>,
    pub attached: bool,
    pub error: Option<String>,
}

impl Server {
    pub async fn start(shares: &[Share], run_dir: &Path) -> Result<Self> {
        let run = run_dir.join("smb");
        state::private_dir(&run)?;
        for directory in ["private", "state", "lock"] {
            state::private_dir(&run.join(directory))?;
        }
        // NSS is scoped to this child, avoiding host accounts or privileged daemons.
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        ensure!(
            uid != 0,
            "run optional shares as an unprivileged lifecycle owner"
        );
        write_private(
            &run.join("passwd"),
            &format!(
                "{USER}:x:{uid}:{gid}::/tmp:/bin/false\nnobody:x:65534:65534::/tmp:/bin/false\n"
            ),
        )?;
        write_private(
            &run.join("group"),
            &format!("{USER}:x:{gid}:\nnogroup:x:65534:\n"),
        )?;
        let password = state::secret();
        write_private(
            &run.join("passwords"),
            &format!(
                "{USER}:{uid}:XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX:{}:[U          ]:LCT-00000001:\n",
                nt_hash(&password)
            ),
        )?;
        let socket = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = socket.local_addr()?.port();
        write_private(&run.join("smb.conf"), &config(shares, &run, port)?)?;
        let listener = UnixListener::bind(run_dir.join("smb.sock"))?;
        drop(socket);
        let executable = std::env::var_os("WDESK_SMBD").unwrap_or_else(|| "smbd".into());
        let child = Command::new(executable)
            .args(["--foreground", "--no-process-group", "--configfile"])
            .arg(run.join("smb.conf"))
            .env("LD_PRELOAD", wrapper()?)
            .env("NSS_WRAPPER_PASSWD", run.join("passwd"))
            .env("NSS_WRAPPER_GROUP", run.join("group"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .context("optional shares require Samba smbd")?;
        let proxy = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(_)=connections.join_next(),if !connections.is_empty() => {},
                    incoming=listener.accept() => {
                        let Ok((mut guest,_))=incoming else {break};
                        if connections.len()>=8 {continue;}
                        connections.spawn(async move {
                            if let Ok(mut server)=TcpStream::connect(("127.0.0.1",port)).await {
                                let _=tokio::io::copy_bidirectional(&mut guest,&mut server).await;
                            }
                        });
                    }
                }
            }
        });
        let group = child.id().context("share server PID unavailable")?;
        let mut server = Self {
            child,
            group,
            run: run.clone(),
            proxy,
            password,
            grants: shares.to_vec(),
            attached: false,
            error: None,
        };
        for _ in 0..100 {
            ensure!(
                server.child.try_wait()?.is_none(),
                "share server exited: {}",
                log_tail(&run)
            );
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return Ok(server);
            }
            sleep(Duration::from_millis(50)).await;
        }
        anyhow::bail!("share server startup deadline: {}", log_tail(&run))
    }

    pub fn attachment(&self) -> Value {
        json!({"op":"owner_shares_attach","args":{"address":ADDRESS,"username":USER,
            "password":self.password,"shares":public(&self.grants)}})
    }

    pub fn public(&self) -> Value {
        json!({"backend":"smb","grants":public(&self.grants),"attached":self.attached,
            "error":self.error,"host_writes_rollback":false})
    }

    pub fn check(&mut self) -> Result<()> {
        ensure!(
            self.child.try_wait()?.is_none(),
            "optional share server exited: {}",
            log_tail(&self.run)
        );
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.proxy.abort();
        // This group was created for this owned child, never an arbitrary PID.
        unsafe {
            libc::kill(-(self.group as i32), libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grants_are_scoped_private_and_identity_checked() {
        let dir = tempfile::tempdir().unwrap();
        let share = grant("source", dir.path(), true).unwrap();
        validate(std::slice::from_ref(&share)).unwrap();
        let public = public(std::slice::from_ref(&share)).to_string();
        assert!(!public.contains(&dir.path().to_string_lossy().to_string()));
        assert!(public.contains("read_only"));
        assert_eq!(
            super::public(std::slice::from_ref(&share))[0]["path"],
            r"\\10.0.2.102\source"
        );
        for name in ["global", "IPC", "../x", "source\n"] {
            assert!(grant(name, dir.path(), true).is_err());
        }
        assert!(grant("source", Path::new("/"), true).is_err());
        let mut stale = share.clone();
        stale.inode += 1;
        assert!(validate(&[stale]).is_err());
        assert!(validate(&[share.clone(), share.clone()]).is_err());
        let cfg = config(&[share], Path::new("/tmp/wdesk-unit"), 18445).unwrap();
        for line in [
            "server signing = mandatory",
            "map to guest = Never",
            "follow symlinks = no",
            "wide links = no",
            "read only = yes",
            "interfaces = 127.0.0.1",
            "oplocks = no",
            "level2 oplocks = no",
            "smb2 leases = no",
        ] {
            assert!(cfg.contains(line));
        }
        for value in ["x\n[evil]", "/tmp/%U", "/tmp/a;comment", " /tmp/a"] {
            assert!(safe_value(value).is_err());
        }
    }
    #[test]
    fn password_hash_matches_nt_password_vector() {
        assert_eq!(nt_hash("password"), "8846F7EAEE8FB117AD06BDD830B7586C");
    }
}
