use crate::{lifecycle, state};
use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub release: &'static str,
    pub architecture: &'static str,
    pub filename: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
    pub profile: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "tiny11-25h2",
        release: "2025-10",
        architecture: "x86_64",
        filename: "tiny11_25H2_Oct25.iso",
        url: "https://archive.org/download/tiny11_25H2/tiny11_25H2_Oct25.iso",
        sha256: "92484f2b7f707e42383294402a9eabbadeaa5ede80ac633390ae7f3537e36275",
        bytes: 5_514_559_488,
        profile: "lite",
    },
    Preset {
        id: "tiny11-core-25h2",
        release: "2025-10",
        architecture: "x86_64",
        filename: "tiny11core_25H2_Oct25.iso",
        url: "https://archive.org/download/tiny11_25H2/tiny11core_25H2_Oct25.iso",
        sha256: "29c055fcfb7b089abd9e007e7abe4bb82c70a03aac9d65e56a38b87ab32d04d2",
        bytes: 3_176_654_848,
        profile: "core",
    },
];

pub fn preset(id: &str) -> Result<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id).ok_or_else(|| {
        anyhow::anyhow!("unknown media preset; use wdesk image media to list choices")
    })
}

fn cached(preset: &Preset, output: &Path) -> Result<bool> {
    if !output.exists() {
        return Ok(false);
    }
    if !output.is_file()
        || output.metadata()?.len() != preset.bytes
        || lifecycle::sha256(output)? != preset.sha256
    {
        bail!("existing media does not match preset; choose another output path");
    }
    Ok(true)
}

pub async fn fetch(id: &str, output: Option<&Path>) -> Result<Value> {
    let preset = preset(id)?;
    let output: PathBuf = if let Some(output) = output {
        output.into()
    } else {
        let dir = state::root().join("media");
        state::private_dir(&dir)?;
        dir.join(preset.filename)
    };
    let _lock = state::lock(
        &state::root()
            .join("media")
            .join(format!(".{}.lock", preset.id)),
    )?;
    if cached(preset, &output)? {
        return Ok(
            json!({"path":output,"sha256":preset.sha256,"bytes":preset.bytes,
            "source":preset.url,"preset":preset.id,"cached":true}),
        );
    }
    let mut result = lifecycle::fetch(preset.url, preset.sha256, &output).await?;
    result["preset"] = json!(preset.id);
    result["cached"] = json!(false);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_distinct_pinned_variants() {
        assert_eq!(PRESETS.len(), 2);
        for preset in PRESETS {
            assert_eq!(preset.sha256.len(), 64);
            assert!(preset.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(preset.url.starts_with("https://"));
            assert!(preset.bytes > 0);
            assert_eq!(Path::new(preset.filename).components().count(), 1);
        }
        assert_eq!(preset("tiny11-25h2").unwrap().profile, "lite");
        assert_eq!(preset("tiny11-core-25h2").unwrap().profile, "core");
        assert!(preset("../unknown").is_err());
        assert_ne!(PRESETS[0].sha256, PRESETS[1].sha256);
    }

    #[test]
    fn cached_media_is_verified_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("media.iso");
        let fixture = Preset {
            id: "fixture",
            release: "test",
            architecture: "x86_64",
            filename: "media.iso",
            url: "https://example.invalid/media.iso",
            sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            bytes: 5,
            profile: "reference",
        };
        assert!(!cached(&fixture, &output).unwrap());
        std::fs::write(&output, b"hello").unwrap();
        assert!(cached(&fixture, &output).unwrap());
        std::fs::write(&output, b"other").unwrap();
        assert!(cached(&fixture, &output).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"other");
        assert!(cached(&fixture, dir.path()).is_err());
    }

    #[tokio::test]
    async fn generic_fetch_cannot_replace_existing_media() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let result = lifecycle::fetch(
            "https://example.invalid/media.iso",
            PRESETS[0].sha256,
            file.path(),
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("output already exists")
        );
    }
}
