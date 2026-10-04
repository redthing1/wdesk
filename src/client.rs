use crate::transfers::{Identity, Range, Start, Transfer};
use crate::{
    protocol::{self, Action, Batch},
    state::{self, Descriptor},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

fn source_range(
    file: Arc<std::fs::File>,
    offset: u64,
    end: u64,
) -> impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> {
    futures_util::stream::try_unfold((file, offset), move |(file, offset)| async move {
        if offset == end {
            return Ok(None);
        }
        let source = file.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            let mut bytes = vec![0; crate::transfers::BUFFER.min((end - offset) as usize)];
            let mut filled = 0;
            while filled < bytes.len() {
                let n = source.read_at(&mut bytes[filled..], offset + filled as u64)?;
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "upload source changed",
                    ));
                }
                filled += n;
            }
            Ok(bytes)
        })
        .await
        .map_err(std::io::Error::other)??;
        let next = offset + bytes.len() as u64;
        Ok(Some((bytes, (file, next))))
    })
}

pub struct Client {
    pub descriptor: Descriptor,
    http: reqwest::Client,
    bulk: reqwest::Client,
}

struct TransferProgress {
    direction: &'static str,
    id: String,
    size: u64,
    offset: u64,
    phase: &'static str,
    started: Instant,
    reported: Instant,
}

impl TransferProgress {
    fn new(direction: &'static str, id: String, size: u64) -> Self {
        let now = Instant::now();
        let mut progress = Self {
            direction,
            id,
            size,
            offset: 0,
            phase: "starting",
            started: now,
            reported: now,
        };
        progress.report();
        progress
    }

    fn update(&mut self, phase: &'static str, offset: u64) {
        let changed = self.phase != phase;
        self.phase = phase;
        self.offset = offset;
        if changed || self.reported.elapsed() >= Duration::from_secs(1) {
            self.report();
        }
    }

    fn report(&mut self) {
        self.reported = Instant::now();
        eprintln!(
            "wdesk {} {}: {} {}/{} bytes ({:.1}s)",
            self.direction,
            self.id,
            self.phase,
            self.offset,
            self.size,
            self.started.elapsed().as_secs_f64()
        );
    }

    fn failure_context(&self) -> String {
        format!(
            "{} {} failed during {} after {}/{} acknowledged bytes ({:.1}s)",
            self.direction,
            self.id,
            self.phase,
            self.offset,
            self.size,
            self.started.elapsed().as_secs_f64()
        )
    }
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
            bulk: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
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
    pub async fn import(&self, local: &Path, remote: &str, resume: Option<&str>) -> Result<Value> {
        match self.binary_identity().await? {
            Some((identity, ranges)) => {
                self.import_binary(local, remote, identity, resume, ranges)
                    .await
            }
            None => {
                ensure!(
                    resume.is_none(),
                    "resume requires a streaming-capable helper"
                );
                eprintln!(
                    "wdesk: older helper; using slow legacy transfer (prepare a new image for streaming)"
                );
                self.import_legacy(local, remote).await
            }
        }
    }
    async fn import_legacy(&self, local: &Path, remote: &str) -> Result<Value> {
        let mut file = std::fs::File::open(local)?;
        let size = file.metadata()?.len();
        ensure!(size <= protocol::MAX_FILE, "file exceeds 4 GiB");
        let transfer = state::id();
        let mut progress = TransferProgress::new("import", transfer.clone(), size);
        let result = async {
            self.guest("file_begin", json!({"path":remote,"transfer":transfer,"size":size})).await?;
            let mut hash = Sha256::new();
            let mut offset = 0;
            let mut buffer = vec![0; protocol::CHUNK];
            progress.update("uploading", 0);
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 { break; }
                hash.update(&buffer[..n]);
                let acknowledged = self.guest("file_write", json!({"transfer":transfer,"offset":offset,"data":STANDARD.encode(&buffer[..n])})).await?;
                ensure!(acknowledged["offset"].as_u64() == Some(offset + n as u64), "invalid import acknowledgment");
                offset += n as u64;
                progress.update("uploading", offset);
            }
            ensure!(offset == size, "local file changed during import");
            progress.update("verifying/committing", offset);
            self.guest("file_commit", json!({"transfer":transfer,"sha256":hex::encode(hash.finalize())})).await
        }.await;
        let failure_context = progress.failure_context();
        if result.is_err() {
            progress.report();
            let _ = self.guest("file_abort", json!({"transfer":transfer})).await;
        } else {
            progress.update("complete", size);
        }
        result.with_context(|| failure_context)
    }
    pub async fn export(&self, remote: &str, local: &Path) -> Result<Value> {
        match self.binary_identity().await? {
            Some((identity, ranges)) => self.export_binary(remote, local, identity, ranges).await,
            None => {
                eprintln!(
                    "wdesk: older helper; using slow legacy transfer (prepare a new image for streaming)"
                );
                self.export_legacy(remote, local).await
            }
        }
    }
    async fn export_legacy(&self, remote: &str, local: &Path) -> Result<Value> {
        eprintln!("wdesk export: inspecting/hashing guest file");
        let meta = self
            .guest("file_stat", json!({"path":remote}))
            .await
            .context("export failed during guest inspection/hash")?;
        let size = meta["size"].as_u64().context("invalid guest file size")?;
        ensure!(size <= protocol::MAX_FILE, "file exceeds 4 GiB");
        let parent = local
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        let mut hash = Sha256::new();
        let mut offset = 0;
        let mut progress = TransferProgress::new("export", state::id(), size);
        progress.update("downloading", 0);
        while offset < size {
            let chunk = self
                .guest(
                    "file_read",
                    json!({"path":remote,"offset":offset,"length":protocol::CHUNK}),
                )
                .await
                .with_context(|| progress.failure_context())?;
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
            progress.update("downloading", offset);
        }
        progress.update("verifying/committing", offset);
        ensure!(
            meta["sha256"] == hex::encode(hash.finalize()),
            "guest file changed or transfer integrity mismatch"
        );
        temp.as_file().sync_all()?;
        temp.persist(local)?;
        progress.update("complete", offset);
        Ok(meta)
    }

    pub async fn file_metadata(&self, path: &str) -> Result<Value> {
        let (identity, _) = self
            .binary_identity()
            .await?
            .context("current binary helper required")?;
        let id = state::id();
        let result = async {
            self.transfer_begin(&Start {
                epoch: identity.epoch.clone(),
                helper_id: identity.helper_id.clone(),
                id: id.clone(),
                direction: "download".into(),
                path: path.into(),
                size: 0,
                sha256: String::new(),
            })
            .await?;
            self.transfer_ready(&id, &identity).await
        }
        .await;
        let released = self.transfer_command(&id, &identity, "abort").await;
        let receipt = result?;
        let mut release = released?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while release.phase == "cancelling" {
            ensure!(
                Instant::now() < deadline,
                "metadata source release deadline"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
            release = self.transfer_status(&id, &identity).await?;
        }
        ensure!(
            release.phase == "cancelled",
            "metadata source was not released"
        );
        Ok(serde_json::to_value(receipt)?)
    }

    async fn binary_identity(&self) -> Result<Option<(Identity, bool)>> {
        let health = self.get("/v1/health").await?;
        if health["guest"]["features"]["binary_files"] != 1 {
            return Ok(None);
        }
        let capabilities = self.get("/v1/capabilities").await?;
        let ranges = capabilities["file_transfer"]["range_identity"] == true;
        Ok(Some((
            Identity {
                epoch: self.descriptor.epoch.clone(),
                helper_id: health["guest"]["helper_id"]
                    .as_str()
                    .context("missing helper incarnation")?
                    .to_owned(),
            },
            ranges,
        )))
    }

    pub async fn transfer(&self, id: &str, cancel: bool) -> Result<Value> {
        ensure!(
            uuid::Uuid::parse_str(id)?.to_string() == id,
            "invalid transfer id"
        );
        let (identity, _) = self
            .binary_identity()
            .await?
            .context("helper does not support binary transfers")?;
        let receipt = if cancel {
            self.transfer_command(id, &identity, "abort").await?
        } else {
            self.transfer_status(id, &identity).await?
        };
        Ok(serde_json::to_value(receipt)?)
    }

    fn transfer_url(&self, id: &str, suffix: &str) -> String {
        format!(
            "{}/v1/transfers/{id}{suffix}",
            self.descriptor.endpoint.trim_end_matches('/')
        )
    }

    async fn transfer_response(response: reqwest::Response) -> Result<Transfer> {
        let status = response.status();
        ensure!(
            status.is_success(),
            "transfer {status}: {}",
            response.text().await?
        );
        Ok(response.json().await?)
    }

    async fn transfer_status(&self, id: &str, identity: &Identity) -> Result<Transfer> {
        let response = self
            .http
            .get(self.transfer_url(id, ""))
            .query(identity)
            .bearer_auth(&self.descriptor.token)
            .send()
            .await?;
        let transfer = Self::transfer_response(response).await?;
        ensure!(
            transfer.id == id && transfer.helper_id == identity.helper_id,
            "stale transfer receipt"
        );
        Ok(transfer)
    }

    async fn transfer_command(
        &self,
        id: &str,
        identity: &Identity,
        command: &str,
    ) -> Result<Transfer> {
        let response = self
            .http
            .post(self.transfer_url(id, &format!("/{command}")))
            .json(identity)
            .bearer_auth(&self.descriptor.token)
            .send()
            .await?;
        Self::transfer_response(response).await
    }

    async fn transfer_begin(&self, request: &Start) -> Result<Transfer> {
        // Begin is idempotent for this incarnation and exact source/destination.
        let mut last = None;
        for _ in 0..3 {
            let result = self
                .post("/v1/transfers", serde_json::to_value(request)?)
                .await
                .and_then(|value| Ok(serde_json::from_value(value)?));
            match result {
                Ok(transfer) => return Ok(transfer),
                Err(error) => last = Some(error),
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(last.expect("begin attempted"))
    }

    fn check_transfer(transfer: &Transfer) -> Result<()> {
        ensure!(
            !["failed", "cancelled", "cancelling"].contains(&transfer.phase.as_str()),
            "transfer {}: {}: {}",
            transfer.id,
            transfer.phase,
            transfer.error.as_deref().unwrap_or("no detail")
        );
        ensure!(
            transfer.offset <= transfer.size && transfer.size <= protocol::MAX_FILE,
            "invalid transfer receipt"
        );
        Ok(())
    }

    async fn transfer_ready(&self, id: &str, identity: &Identity) -> Result<Transfer> {
        let deadline = Instant::now() + Duration::from_secs(600);
        let mut delay = Duration::from_millis(25);
        loop {
            let transfer = self.transfer_status(id, identity).await?;
            Self::check_transfer(&transfer)?;
            if transfer.phase == "ready" && !transfer.active {
                return Ok(transfer);
            }
            ensure!(
                Instant::now() < deadline,
                "transfer preparation deadline exceeded"
            );
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_millis(250));
        }
    }

    async fn transfer_reconcile(&self, id: &str, identity: &Identity) -> Result<Transfer> {
        // A failed stream may have delivered bytes. Interrupt its framing, then
        // use the worker's accepted offset, never the host's attempted offset.
        self.transfer_command(id, identity, "pause").await?;
        self.transfer_ready(id, identity).await
    }

    async fn transfer_commit(&self, id: &str, identity: &Identity) -> Result<Transfer> {
        let deadline = Instant::now() + Duration::from_secs(600);
        let mut retries = 0;
        let mut delay = Duration::from_millis(25);
        loop {
            let mut transfer = self.transfer_status(id, identity).await?;
            Self::check_transfer(&transfer)?;
            if transfer.phase == "committed" {
                return Ok(transfer);
            }
            if transfer.phase == "ready" && !transfer.active {
                retries += 1;
                ensure!(
                    retries <= 3,
                    "commit acknowledgment unavailable; inspect transfer {id}"
                );
                // Only this idempotent transfer commit is retryable; a general
                // guest mutation is never automatically repeated.
                if let Ok(receipt) = self.transfer_command(id, identity, "commit").await {
                    transfer = receipt;
                }
                Self::check_transfer(&transfer)?;
                if transfer.phase == "committed" {
                    return Ok(transfer);
                }
            }
            ensure!(
                Instant::now() < deadline,
                "transfer verification deadline exceeded"
            );
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_millis(250));
        }
    }

    async fn import_binary(
        &self,
        local: &Path,
        remote: &str,
        identity: Identity,
        resume: Option<&str>,
        ranges: bool,
    ) -> Result<Value> {
        let file = std::fs::File::open(local)?;
        let metadata = file.metadata()?;
        let size = metadata.len();
        ensure!(
            metadata.is_file() && size <= protocol::MAX_FILE,
            "import needs a regular file of at most 4 GiB"
        );
        let id = resume.map(str::to_owned).unwrap_or_else(state::id);
        ensure!(
            uuid::Uuid::parse_str(&id)?.to_string() == id,
            "invalid transfer id"
        );
        let mut progress = TransferProgress::new("import", id.clone(), size);
        progress.update("hashing source", 0);
        let hash_file = file.try_clone()?;
        let expected = tokio::task::spawn_blocking(move || -> Result<String> {
            let mut file = hash_file;
            let mut hash = Sha256::new();
            let mut buffer = vec![0; crate::transfers::BUFFER];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            Ok(hex::encode(hash.finalize()))
        })
        .await??;
        let mut owns_transfer = resume.is_none();
        let result = async {
            let mut started = if resume.is_some() {
                self.transfer_status(&id,&identity).await?
            } else {
                self.transfer_begin(&Start { epoch:identity.epoch.clone(),helper_id:identity.helper_id.clone(),id:id.clone(),direction:"upload".into(),path:remote.into(),size,sha256:expected.clone() }).await?
            };
            Self::check_transfer(&started)?;
            ensure!(started.id==id && started.helper_id==identity.helper_id && started.direction=="upload" && started.path==remote && started.size==size && started.sha256.as_deref()==Some(expected.as_str()),"resume source/destination differs from original transfer");
            owns_transfer=true;
            if started.active {started=self.transfer_reconcile(&id,&identity).await?;}
            let source = Arc::new(file);
            let mut offset = started.offset;
            let mut attempts = 0;
            while offset < size {
                let range = Range { epoch:identity.epoch.clone(),helper_id:identity.helper_id.clone(),offset,length:size-offset,request_id:ranges.then(state::id) };
                // Positional reads avoid a shared seek cursor between an old
                // cancelled HTTP producer and its reconnecting replacement.
                let reader = source_range(source.clone(), offset, size);
                let request = self.bulk.put(self.transfer_url(&id, "/data")).query(&range)
                    .bearer_auth(&self.descriptor.token).header(reqwest::header::CONTENT_LENGTH,size-offset)
                    .body(reqwest::Body::wrap_stream(reader)).send();
                let mut request=Box::pin(request);
                progress.update("uploading", offset);
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                let mut accepted = offset;
                let mut advanced = Instant::now();
                let response = loop {
                    tokio::select! {
                        result = &mut request => break result.map_err(anyhow::Error::from),
                        _ = tick.tick() => {
                            let receipt = self.transfer_status(&id,&identity).await?;
                            Self::check_transfer(&receipt)?;
                            if receipt.range_interrupted(&range) {break Err(anyhow::anyhow!("guest range interrupted: {}",receipt.error.as_deref().unwrap_or("no detail")));}
                            if receipt.offset > accepted { accepted = receipt.offset;advanced = Instant::now(); }
                            ensure!(advanced.elapsed() < Duration::from_secs(30),"upload acknowledgment idle deadline");
                            progress.update("uploading",receipt.offset);
                        }
                    }
                };
                drop(request); // Cancel this HTTP attempt before reconciling.
                let completed = match response {
                    Ok(response) if response.status().is_success() => {
                        match response.json::<Value>().await {
                            Ok(reply) => {ensure!(reply["offset"] == size, "invalid stream acknowledgment");true},
                            Err(error) => {eprintln!("wdesk import {id}: lost stream acknowledgment: {error}");false},
                        }
                    }
                    Ok(response) => { eprintln!("wdesk import {id}: interrupted: {}",response.status()); false }
                    Err(error) => { eprintln!("wdesk import {id}: interrupted: {error}"); false }
                };
                if completed { offset = size; } else {
                    attempts += 1;
                    ensure!(attempts <= 3, "stream retry limit exceeded");
                    offset = self.transfer_reconcile(&id,&identity).await?.offset;
                    progress.update("resuming",offset);
                }
            }
            progress.update("verifying/committing", size);
            let committed = self.transfer_commit(&id,&identity).await?;
            Ok(json!({"path":committed.path,"size":committed.size,"sha256":committed.sha256,"transfer":id,"transport":"binary_stream"}))
        }.await;
        if result.is_err() {
            progress.report();
            if owns_transfer {
                let _ = self.transfer_command(&id, &identity, "abort").await;
            }
        } else {
            progress.update("complete", size);
        }
        result.with_context(|| progress.failure_context())
    }

    async fn export_binary(
        &self,
        remote: &str,
        local: &Path,
        identity: Identity,
        ranges: bool,
    ) -> Result<Value> {
        let id = state::id();
        let mut progress = TransferProgress::new("export", id.clone(), 0);
        let result = async {
            let begin = self.transfer_begin(&Start { epoch:identity.epoch.clone(),helper_id:identity.helper_id.clone(),id:id.clone(),direction:"download".into(),path:remote.into(),size:0,sha256:String::new() }).await?;
            progress.size = begin.size;
            progress.update("hashing guest source",0);
            let transfer = self.transfer_ready(&id,&identity).await?;
            let size = transfer.size;
            let parent = local.parent().filter(|p|!p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            let mut hash = Sha256::new();
            let mut offset = 0;
            let mut attempts = 0;
            while offset < size {
                progress.update("downloading",offset);
                let range = Range { epoch:identity.epoch.clone(),helper_id:identity.helper_id.clone(),offset,length:size-offset,request_id:ranges.then(state::id) };
                let attempt = async {
                    let response = self.bulk.get(self.transfer_url(&id,"/data")).query(&range)
                        .bearer_auth(&self.descriptor.token).send().await?;
                    ensure!(response.status().is_success(), "download {}: {}",response.status(),response.text().await?);
                    let mut stream = response.bytes_stream();
                    let mut tick=tokio::time::interval(Duration::from_secs(1));
                    let mut received_at=Instant::now();
                    loop {
                        tokio::select! {
                            bytes=stream.next()=> {
                                let Some(bytes)=bytes else {break;};let bytes=bytes?;
                                ensure!(bytes.len() as u64 <= size-offset,"download exceeds declared size");
                                temp.write_all(&bytes)?;hash.update(&bytes);offset += bytes.len() as u64;
                                received_at=Instant::now();progress.update("downloading",offset);
                            }
                            _=tick.tick()=> {
                                ensure!(received_at.elapsed()<Duration::from_secs(30),"download idle deadline");
                                if ranges {
                                    let receipt=self.transfer_status(&id,&identity).await?;
                                    Self::check_transfer(&receipt)?;
                                    ensure!(!receipt.range_interrupted(&range),"guest range interrupted: {}",receipt.error.as_deref().unwrap_or("no detail"));
                                }
                            }
                        }
                    }
                    ensure!(offset == size,"download ended early");
                    Ok::<_,anyhow::Error>(())
                }.await;
                if let Err(error) = attempt {
                    // A partial local write is not a reconnectable network
                    // failure. Never persist a possibly torn temporary file.
                    if error.downcast_ref::<std::io::Error>().is_some() {return Err(error);}
                    attempts += 1;
                    ensure!(attempts <= 3,"download retry limit exceeded: {error:#}");
                    eprintln!("wdesk export {id}: resuming at {offset} bytes: {error:#}");
                    self.transfer_reconcile(&id,&identity).await?;
                }
            }
            progress.update("verifying/committing",offset);
            ensure!(transfer.sha256.as_deref() == Some(hex::encode(hash.finalize()).as_str()),"download SHA-256 mismatch; local destination unchanged");
            self.transfer_commit(&id,&identity).await?;
            temp.as_file().sync_all()?;temp.persist(local)?;
            Ok(json!({"path":remote,"size":size,"sha256":transfer.sha256,"transfer":id,"transport":"binary_stream"}))
        }.await;
        if result.is_err() {
            progress.report();
            let _ = self.transfer_command(&id, &identity, "abort").await;
        } else {
            progress.update("complete", progress.size);
        }
        result.with_context(|| progress.failure_context())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn retry_sources_have_independent_positions_and_bounded_chunks() {
        use futures_util::TryStreamExt;
        use std::io::{Seek, SeekFrom};
        let mut file = tempfile::tempfile().unwrap();
        let bytes: Vec<u8> = (0..crate::transfers::BUFFER * 4 + 17)
            .map(|i| (i % 251) as u8)
            .collect();
        file.write_all(&bytes).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        let file = Arc::new(file);
        let end = bytes.len() as u64;
        let (old, retry) = tokio::join!(
            source_range(file.clone(), 0, end).try_collect::<Vec<_>>(),
            source_range(file, 123, end).try_collect::<Vec<_>>(),
        );
        let old = old.unwrap();
        let retry = retry.unwrap();
        assert!(
            old.iter()
                .chain(&retry)
                .all(|chunk| chunk.len() <= crate::transfers::BUFFER)
        );
        assert_eq!(old.into_iter().flatten().collect::<Vec<_>>(), bytes);
        assert_eq!(
            retry.into_iter().flatten().collect::<Vec<_>>(),
            bytes[123..]
        );
    }
    use axum::{Json, Router, extract::State, routing::post};
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn import_rejects_wrong_ack_and_aborts_without_committing() {
        async fn handle(
            State(operations): State<Arc<Mutex<Vec<String>>>>,
            Json(request): Json<Value>,
        ) -> Json<Value> {
            let operation = request["op"].as_str().unwrap().to_owned();
            operations.lock().unwrap().push(operation.clone());
            Json(match operation.as_str() {
                "file_begin" => json!({"offset":0}),
                "file_write" => json!({"offset":1}),
                "file_abort" => json!({"aborted":true}),
                _ => panic!("unexpected guest operation: {operation}"),
            })
        }
        let operations = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new()
            .route("/v1/guest", post(handle))
            .with_state(operations.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::new(Descriptor {
            protocol: 1,
            endpoint: format!("http://{address}"),
            token: "test-token".into(),
            epoch: "test-epoch".into(),
        })
        .unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let source = scratch.path().join("input.bin");
        std::fs::write(&source, b"abc").unwrap();
        let failure = client
            .import_legacy(&source, "workspace/input.bin")
            .await
            .unwrap_err();
        let message = format!("{failure:#}");
        assert!(message.contains("during uploading after 0/3 acknowledged bytes"));
        assert!(message.contains("invalid import acknowledgment"));
        assert_eq!(
            *operations.lock().unwrap(),
            ["file_begin", "file_write", "file_abort"]
        );
        server.abort();
    }
}
