use crate::{
    protocol::{self, Action, Batch},
    state::{self, Descriptor},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Client {
    pub descriptor: Descriptor,
    http: reqwest::Client,
}
impl Client {
    pub fn new(descriptor: Descriptor) -> Result<Self> {
        ensure!(descriptor.protocol == 1, "unsupported descriptor protocol");
        let url = reqwest::Url::parse(&descriptor.endpoint)?;
        ensure!(
            ["http", "https"].contains(&url.scheme())
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "invalid data-plane endpoint"
        );
        Ok(Self {
            descriptor,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(90))
                .no_proxy()
                .build()?,
        })
    }
    pub fn discover(session: &str, descriptor: Option<&Path>) -> Result<Self> {
        let path = if let Some(path) = descriptor {
            path.to_path_buf()
        } else if let Some(path) = std::env::var_os("WDESK_DESCRIPTOR") {
            PathBuf::from(path)
        } else {
            state::validate_name(session)?;
            state::root()
                .join("sessions")
                .join(session)
                .join("client.json")
        };
        Self::new(state::read_json(&path).with_context(|| {
            format!(
                "no session descriptor at {}; run wdesk open",
                path.display()
            )
        })?)
    }
    async fn response(&self, path: &str, body: Option<Value>) -> Result<reqwest::Response> {
        let url = format!("{}{path}", self.descriptor.endpoint.trim_end_matches('/'));
        let request = if let Some(body) = body {
            self.http.post(url).json(&body)
        } else {
            self.http.get(url)
        };
        let response = request
            .bearer_auth(&self.descriptor.token)
            .send()
            .await
            .context("connecting to Windows runtime")?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("runtime {status}: {}", response.text().await?);
        }
        Ok(response)
    }
    pub async fn get(&self, path: &str) -> Result<Value> {
        Ok(self.response(path, None).await?.json().await?)
    }
    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        Ok(self.response(path, Some(body)).await?.json().await?)
    }
    pub async fn guest(&self, op: &str, args: Value) -> Result<Value> {
        self.post("/v1/guest", json!({"op":op,"args":args})).await
    }
    pub async fn action(&self, actions: Vec<Action>, expected: Option<u64>) -> Result<Value> {
        let batch = Batch {
            protocol: 1,
            request_id: state::id(),
            epoch: self.descriptor.epoch.clone(),
            expected_input_generation: expected,
            actions,
        };
        let result = self.post("/v1/batch", serde_json::to_value(batch)?).await?;
        ensure!(
            result["results"]
                .as_array()
                .is_some_and(|r| r.iter().all(|r| r["delivered"] == true)),
            "partial action delivery: {result}"
        );
        Ok(result)
    }
    pub async fn screenshot(&self, output: &Path) -> Result<Value> {
        let response = self.response("/v1/see", None).await?;
        let metadata: Value = serde_json::from_str(
            response
                .headers()
                .get("x-wdesk-observation")
                .context("missing screenshot metadata")?
                .to_str()?,
        )?;
        let bytes = response.bytes().await?;
        ensure!(
            metadata["sha256"] == hex::encode(Sha256::digest(&bytes)),
            "screenshot integrity mismatch"
        );
        atomic_write(output, &bytes)?;
        Ok(metadata)
    }
    pub async fn import(&self, local: &Path, remote: &str) -> Result<Value> {
        let mut file = std::fs::File::open(local)?;
        let size = file.metadata()?.len();
        ensure!(size <= protocol::MAX_FILE, "file exceeds 4 GiB");
        let transfer = state::id();
        self.guest(
            "file_begin",
            json!({"path":remote,"transfer":transfer,"size":size}),
        )
        .await?;
        let result=async {
            let mut hash=Sha256::new();let mut offset=0;let mut buffer=vec![0;protocol::CHUNK];
            loop {
                let n=file.read(&mut buffer)?;if n==0{break;}
                hash.update(&buffer[..n]);
                self.guest("file_write",json!({"transfer":transfer,"offset":offset,"data":STANDARD.encode(&buffer[..n])})).await?;
                offset+=n as u64;
            }
            ensure!(offset==size,"local file changed during import");
            self.guest("file_commit",json!({"transfer":transfer,"sha256":hex::encode(hash.finalize())})).await
        }.await;
        if result.is_err() {
            let _ = self.guest("file_abort", json!({"transfer":transfer})).await;
        }
        result
    }
    pub async fn export(&self, remote: &str, local: &Path) -> Result<Value> {
        let meta = self.guest("file_stat", json!({"path":remote})).await?;
        let size = meta["size"].as_u64().context("invalid guest file size")?;
        ensure!(size <= protocol::MAX_FILE, "file exceeds 4 GiB");
        let parent = local
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        let mut hash = Sha256::new();
        let mut offset = 0;
        while offset < size {
            let chunk = self
                .guest(
                    "file_read",
                    json!({"path":remote,"offset":offset,"length":protocol::CHUNK}),
                )
                .await?;
            let bytes = STANDARD.decode(chunk["data"].as_str().context("missing file data")?)?;
            ensure!(
                !bytes.is_empty()
                    && bytes.len() <= protocol::CHUNK
                    && offset + bytes.len() as u64 <= size,
                "invalid export chunk"
            );
            hash.update(&bytes);
            temp.write_all(&bytes)?;
            offset += bytes.len() as u64;
        }
        ensure!(
            meta["sha256"] == hex::encode(hash.finalize()),
            "guest file changed or transfer integrity mismatch"
        );
        temp.as_file().sync_all()?;
        temp.persist(local)?;
        Ok(meta)
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
