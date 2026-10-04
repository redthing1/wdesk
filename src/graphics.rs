//! One optional, pinned software preset. Never run vendor deployment scripts.
use crate::{client::Client, lifecycle, state};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

const PRESET: &str = "software-v1";
const CACHE: &str = "workspace/.wdesk/graphics/software-v1";
struct Artifact {
    name: &'static str,
    url: &'static str,
    sha256: &'static str,
    bytes: u64,
}
const MESA: Artifact = Artifact {
    name: "mesa3d-26.2.2-release-msvc.7z",
    url: "https://github.com/pal1000/mesa-dist-win/releases/download/26.2.2/mesa3d-26.2.2-release-msvc.7z",
    sha256: "0f21394d44036019bb5c07a3788dd278fd7b7b3469077a4686f0d639e8e874a1",
    bytes: 71_035_935,
};
const VULKAN: Artifact = Artifact {
    name: "vulkan-runtime-components-1.4.363.0.zip",
    url: "https://sdk.lunarg.com/sdk/download/1.4.363.0/windows/vulkan-runtime-components.zip",
    sha256: "a25a927aa8b9f0371048f1861cf88ac3b9bc9b1fb332c42d897c8ab32695769a",
    bytes: 18_211_191,
};
const VULKAN_DIR: &str = "VulkanRT-X64-1.4.363.0-Components";

pub fn components(arch: &str, apis: &[String]) -> Result<Vec<String>> {
    ensure!(["x64", "x86"].contains(&arch), "select x64 or x86");
    ensure!(
        !apis.is_empty()
            && apis.len() <= 3
            && apis
                .iter()
                .all(|api| ["gl", "gles", "vk"].contains(&api.as_str())),
        "select gl, gles, vk"
    );
    let mut files = Vec::new();
    let mut add = |name: &str| {
        if !files.iter().any(|entry| entry == name) {
            files.push(name.to_owned());
        }
    };
    if apis.iter().any(|api| api == "gl" || api == "gles") {
        add("opengl32.dll");
        add("libgallium_wgl.dll");
    }
    if apis.iter().any(|api| api == "gles") {
        for name in ["libEGL.dll", "libGLESv1_CM.dll", "libGLESv2.dll"] {
            add(name);
        }
    }
    if apis.iter().any(|api| api == "vk") {
        add("vulkan_lvp.dll");
        add("vulkan-1.dll");
        add(if arch == "x64" {
            "lvp_icd.x86_64.json"
        } else {
            "lvp_icd.x86.json"
        });
    }
    Ok(files)
}

async fn supported(client: &Client) -> Result<()> {
    let health = client.get("/v1/health").await?;
    ensure!(
        health["guest"]["features"]["software_graphics"] == 1,
        "prepare a current helper image for optional graphics setup"
    );
    Ok(())
}
pub async fn status(client: &Client, arch: &str, apis: &[String]) -> Result<Value> {
    components(arch, apis)?;
    supported(client).await?;
    client
        .guest("graphics_status", json!({"architecture":arch,"apis":apis}))
        .await
}
pub async fn prepare(client: &Client, path: &str, apis: &[String]) -> Result<Value> {
    components("x64", apis)?;
    supported(client).await?;
    client
        .guest("graphics_prepare", json!({"path":path,"apis":apis}))
        .await
}
pub async fn run(
    client: &Client,
    path: &str,
    apis: &[String],
    argv: &[String],
    seconds: u64,
) -> Result<Value> {
    components("x64", apis)?;
    supported(client).await?;
    ensure!(
        (1..=3600).contains(&seconds) && argv.len() < 128,
        "invalid owned graphics process options"
    );
    client
        .guest(
            "graphics_run",
            json!({"path":path,"apis":apis,"argv":argv,"timeout_seconds":seconds}),
        )
        .await
}

async fn fetch(artifact: &Artifact, root: &Path) -> Result<PathBuf> {
    let path = root.join(artifact.name);
    if path.exists() {
        ensure!(
            path.is_file()
                && path.metadata()?.len() == artifact.bytes
                && lifecycle::sha256(&path)? == artifact.sha256,
            "cached graphics archive does not match its pin: {}",
            path.display()
        );
    } else {
        eprintln!(
            "wdesk graphics: fetching {} ({} bytes)",
            artifact.name, artifact.bytes
        );
        lifecycle::fetch_limited(artifact.url, artifact.sha256, &path, artifact.bytes).await?;
        ensure!(
            path.metadata()?.len() == artifact.bytes,
            "graphics archive size mismatch"
        );
    }
    Ok(path)
}
async fn extract(archive: &Path, root: &Path, files: &[String]) -> Result<()> {
    let mut child = Command::new("7z")
        .args(["x", "-y", "-bd", "-bso0", "-bsp0"])
        .arg(format!("-o{}", root.display()))
        .arg(archive)
        .args(files)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("optional graphics installation requires 7z")?;
    let mut stderr = child.stderr.take().context("7z error pipe")?;
    let errors = tokio::spawn(async move {
        let mut kept = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let n = stderr.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            let retain = n.min(8192 - kept.len());
            kept.extend_from_slice(&buffer[..retain]);
        }
        Ok::<_, std::io::Error>(kept)
    });
    let result = timeout(Duration::from_secs(180), child.wait()).await;
    if result.is_err() {
        child.kill().await?;
    }
    let errors = errors.await??;
    ensure!(
        result.context("graphics extraction deadline")??.success(),
        "graphics extraction: {}",
        String::from_utf8_lossy(&errors)
    );
    for file in files {
        ensure!(
            root.join(file).symlink_metadata()?.file_type().is_file(),
            "unexpected graphics archive member"
        );
    }
    Ok(())
}

pub async fn install(client: &Client, arch: &str, apis: &[String]) -> Result<Value> {
    let files = components(arch, apis)?;
    let previous = status(client, arch, apis).await?;
    let root = state::root().join("cache/graphics");
    state::private_dir(&root)?;
    let _lock = state::lock(&root.join("software-v1.lock"))?;
    let temp = tempfile::Builder::new()
        .prefix("unpack-")
        .tempdir_in(&root)?;
    let mut sources = BTreeMap::new();
    let mesa_files: Vec<_> = files
        .iter()
        .filter(|name| name.as_str() != "vulkan-1.dll")
        .map(|name| format!("{arch}/{name}"))
        .collect();
    let archive = fetch(&MESA, &root).await?;
    extract(&archive, temp.path(), &mesa_files).await?;
    for file in &files {
        if file != "vulkan-1.dll" {
            sources.insert(file.clone(), temp.path().join(arch).join(file));
        }
    }
    if files.iter().any(|file| file == "vulkan-1.dll") {
        let archive = fetch(&VULKAN, &root).await?;
        let loader = format!("{VULKAN_DIR}/{arch}/vulkan-1.dll");
        let license = format!("{VULKAN_DIR}/VulkanRT-License.txt");
        extract(&archive, temp.path(), &[loader.clone(), license.clone()]).await?;
        sources.insert("vulkan-1.dll".into(), temp.path().join(loader));
        sources.insert("VulkanRT-License.txt".into(), temp.path().join(license));
    }
    let mut inventory = Vec::new();
    let mut bytes = 0u64;
    let mut uploaded = 0u64;
    for (name, source) in sources {
        let sha = lifecycle::sha256(&source)?;
        let size = source.metadata()?.len();
        bytes += size;
        let remote = format!("{CACHE}/{arch}/{name}");
        let present = previous["files"].as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["name"] == name && entry["present"] == true)
        });
        let unchanged = if present {
            let meta = client.file_metadata(&remote).await?;
            meta["size"] == size && meta["sha256"] == sha
        } else {
            false
        };
        if !unchanged {
            client.import(&source, &remote, None).await?;
            uploaded += size;
        }
        inventory.push(json!({"name":name,"bytes":size,"sha256":sha,"reused":unchanged}));
    }
    let manifest = json!({"preset":PRESET,"architecture":arch,"apis":apis,"files":inventory,
        "sources":[{"url":MESA.url,"sha256":MESA.sha256},{"url":VULKAN.url,"sha256":VULKAN.sha256}],
        "notices":[{"project":"Mesa","license_url":"https://docs.mesa3d.org/license.html"},
            {"project":"LLVM","license_url":"https://llvm.org/LICENSE.txt"}],"render_verified":false});
    let path = temp.path().join("manifest.json");
    state::write_json(&path, &manifest)?;
    client
        .import(&path, &format!("{CACHE}/{arch}/manifest.json"), None)
        .await?;
    Ok(
        json!({"preset":PRESET,"architecture":arch,"apis":apis,"bytes":bytes,"uploaded_bytes":uploaded,"files":inventory,"render_verified":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preset_is_bounded_arch_specific_and_composable() {
        assert!(components("../x64", &["gl".into()]).is_err());
        assert!(components("x64", &["anything".into()]).is_err());
        assert!(components("x64", &[]).is_err());
        assert_eq!(components("x64", &["gl".into()]).unwrap().len(), 2);
        assert_eq!(components("x64", &["gles".into()]).unwrap().len(), 5);
        let all = components("x86", &["gl".into(), "gles".into(), "vk".into()]).unwrap();
        assert_eq!(all.len(), 8);
        assert!(all.contains(&"lvp_icd.x86.json".into()));
        assert!(!all.contains(&"lvp_icd.x86_64.json".into()));
        for artifact in [&MESA, &VULKAN] {
            assert_eq!(artifact.sha256.len(), 64);
            assert!(artifact.bytes < 80 * 1024 * 1024);
            assert!(artifact.url.starts_with("https://"));
        }
    }
}
