//! Authenticated transfer control and one bounded binary lane per VM.
use crate::{
    protocol::{MAX_FILE, OperationOutcome},
    runtime::{self, Runtime},
    state,
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::{OwnedMutexGuard, watch},
    time::timeout,
};

pub const BUFFER: usize = 256 * 1024;
const IDLE: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Channel {
    stream: Option<UnixStream>,
    abandoned: Option<(String, String)>,
}

// Independent of the lane mutex: control can release a stalled HTTP body or
// guest read without waiting for the very stream it is trying to interrupt.
pub struct Interrupt {
    id: String,
    helper: String,
    signal: watch::Sender<bool>,
}
fn register_interrupt(runtime: &Runtime, id: &str, helper: &str) -> watch::Receiver<bool> {
    let (signal, receiver) = watch::channel(false);
    *runtime.bulk_interrupt.lock().expect("bulk interrupt lock") = Some(Interrupt {
        id: id.into(),
        helper: helper.into(),
        signal,
    });
    receiver
}
fn interrupt(runtime: &Runtime, id: &str, helper: &str) {
    let guard = runtime.bulk_interrupt.lock().expect("bulk interrupt lock");
    if let Some(active) = guard
        .as_ref()
        .filter(|active| active.id == id && active.helper == helper)
    {
        active.signal.send_replace(true);
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub epoch: String,
    pub helper_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub epoch: String,
    pub helper_id: String,
    pub id: String,
    pub direction: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub epoch: String,
    pub helper_id: String,
    pub offset: u64,
    pub length: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transfer {
    pub id: String,
    pub helper_id: String,
    pub path: String,
    pub direction: String,
    pub size: u64,
    pub offset: u64,
    pub sha256: Option<String>,
    pub phase: String,
    pub active: bool,
    #[serde(default)]
    pub range_id: Option<String>,
    pub error: Option<String>,
    pub durable: bool,
}

impl Transfer {
    pub fn range_interrupted(&self, range: &Range) -> bool {
        range
            .request_id
            .as_ref()
            .is_some_and(|id| self.range_id.as_ref() == Some(id))
            && self.phase == "ready"
            && !self.active
            && self.error.is_some()
    }
}

pub fn routes() -> Router<Arc<Runtime>> {
    Router::new()
        .route("/v1/transfers", post(start))
        .route("/v1/transfers/{id}", get(status))
        .route("/v1/transfers/{id}/commit", post(commit))
        .route("/v1/transfers/{id}/abort", post(abort))
        .route("/v1/transfers/{id}/pause", post(pause))
        .route(
            "/v1/transfers/{id}/data",
            get(download).put(upload).layer(DefaultBodyLimit::disable()),
        )
}

fn valid_id(id: &str) -> Result<()> {
    ensure!(
        uuid::Uuid::parse_str(id)?.to_string() == id,
        "invalid transfer id"
    );
    Ok(())
}

async fn validate(runtime: &Runtime, epoch: &str, helper: &str) -> Result<()> {
    ensure!(
        epoch == runtime.descriptor.epoch,
        "runtime restarted; transfer expired"
    );
    valid_id(helper)?;
    let health = runtime.health.read().await;
    ensure!(
        health["guest"]["helper_id"] == helper,
        "helper incarnation is not ready"
    );
    ensure!(
        health["guest"]["features"]["binary_files"] == 1,
        "helper does not support binary transfers"
    );
    Ok(())
}

fn failure(error: anyhow::Error, dispatched: bool) -> Response {
    let rejected = error.downcast_ref::<crate::qmp::GuestFailure>().is_some();
    runtime::operation_error(
        if dispatched {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::CONFLICT
        },
        if rejected {
            "guest_rejected"
        } else if dispatched {
            "transfer_interrupted"
        } else {
            "transfer_not_started"
        },
        format!("{error:#}"),
        dispatched && !rejected,
        if dispatched {
            OperationOutcome::Unknown
        } else {
            OperationOutcome::NotStarted
        },
    )
}

async fn control(runtime: &Runtime, op: &str, args: Value) -> Result<Value> {
    let _guard = runtime.guest_lock.lock().await;
    runtime.guest_request(json!({"op":op,"args":args})).await
}

async fn start(
    State(runtime): State<Arc<Runtime>>,
    headers: HeaderMap,
    Json(request): Json<Start>,
) -> Response {
    if !runtime::authorized(&runtime, &headers, false) {
        return runtime::unauthorized();
    }
    let checked = async {
        validate(&runtime, &request.epoch, &request.helper_id).await?;
        valid_id(&request.id)?;
        ensure!(request.size <= MAX_FILE, "file exceeds 4 GiB");
        ensure!(
            ["upload", "download"].contains(&request.direction.as_str()),
            "invalid direction"
        );
        Ok(())
    }
    .await;
    if let Err(error) = checked {
        return failure(error, false);
    }
    match control(&runtime, "transfer_begin", json!({"transfer":request.id,"helper_id":request.helper_id,"direction":request.direction,"path":request.path,"size":request.size,"sha256":request.sha256})).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => failure(error, true),
    }
}

async fn status(
    State(runtime): State<Arc<Runtime>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(identity): Query<Identity>,
) -> Response {
    command(runtime, headers, id, identity, "transfer_status").await
}
async fn commit(
    State(runtime): State<Arc<Runtime>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(identity): Json<Identity>,
) -> Response {
    command(runtime, headers, id, identity, "transfer_commit").await
}
async fn abort(
    State(runtime): State<Arc<Runtime>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(identity): Json<Identity>,
) -> Response {
    command(runtime, headers, id, identity, "transfer_abort").await
}
async fn pause(
    State(runtime): State<Arc<Runtime>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(identity): Json<Identity>,
) -> Response {
    command(runtime, headers, id, identity, "transfer_pause").await
}
async fn command(
    runtime: Arc<Runtime>,
    headers: HeaderMap,
    id: String,
    identity: Identity,
    op: &str,
) -> Response {
    if !runtime::authorized(&runtime, &headers, false) {
        return runtime::unauthorized();
    }
    if let Err(error) = async {
        valid_id(&id)?;
        validate(&runtime, &identity.epoch, &identity.helper_id).await
    }
    .await
    {
        return failure(error, false);
    }
    if matches!(op, "transfer_pause" | "transfer_abort") {
        interrupt(&runtime, &id, &identity.helper_id);
    }
    match control(
        &runtime,
        op,
        json!({"transfer":id,"helper_id":identity.helper_id}),
    )
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(error) => failure(error, true),
    }
}

/// A dropped or failed range poisons its framing; it cannot be reused blindly.
struct Lane {
    channel: OwnedMutexGuard<Channel>,
    cancelled: watch::Receiver<bool>,
    nonce: String,
    ready_nonce: Option<String>,
    id: String,
    helper: String,
    reusable: bool,
}
impl Drop for Lane {
    fn drop(&mut self) {
        if !self.reusable {
            self.channel.stream.take();
            self.channel.abandoned = Some((self.id.clone(), self.helper.clone()));
        }
    }
}
impl Lane {
    fn stream(&mut self) -> &mut UnixStream {
        self.channel.stream.as_mut().expect("connected lane")
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(!*self.cancelled.borrow(), "bulk range interrupted");
        let mut cancelled = self.cancelled.clone();
        tokio::select! {
            biased;
            _ = cancelled.changed() => anyhow::bail!("bulk range interrupted"),
            result = timeout(IDLE, self.stream().write_all(bytes)) => result.context("bulk write idle deadline")??,
        }
        Ok(())
    }
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize> {
        ensure!(!*self.cancelled.borrow(), "bulk range interrupted");
        let mut cancelled = self.cancelled.clone();
        tokio::select! {
            biased;
            _ = cancelled.changed() => anyhow::bail!("bulk range interrupted"),
            result = timeout(IDLE, self.stream().read(bytes)) => Ok(result.context("bulk read idle deadline")??),
        }
    }
    async fn exact(&mut self, bytes: &mut [u8]) -> Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            let n = self.read(&mut bytes[offset..]).await?;
            ensure!(n != 0, "bulk channel ended early");
            offset += n;
        }
        Ok(())
    }
    async fn reply(&mut self, offset: u64) -> Result<()> {
        for _ in 0..12 {
            let mut header = [0u8; 53];
            self.exact(&mut header).await?;
            if self
                .ready_nonce
                .as_deref()
                .is_some_and(|nonce| header[4..40] == *nonce.as_bytes())
            {
                ensure!(
                    &header[..4] == b"WDB1" && header[40..53].iter().all(|b| *b == 0),
                    "invalid readiness reply"
                );
                continue;
            }
            ensure!(
                &header[..4] == b"WDB1" && &header[4..40] == self.nonce.as_bytes(),
                "stale or invalid bulk reply"
            );
            let accepted = u64::from_le_bytes(header[41..49].try_into()?);
            let length = u32::from_le_bytes(header[49..53].try_into()?) as usize;
            ensure!(length <= 4096, "oversized bulk error");
            let mut message = vec![0; length];
            self.exact(&mut message).await?;
            if header[40] != 0 {
                return Err(crate::qmp::GuestFailure(
                    String::from_utf8_lossy(&message).into_owned(),
                )
                .into());
            }
            ensure!(
                accepted == offset && length == 0,
                "invalid bulk acknowledgment"
            );
            return Ok(());
        }
        anyhow::bail!("too many readiness replies")
    }

    async fn probe(&mut self) -> Result<()> {
        let nonce = state::id();
        self.ready_nonce = Some(nonce.clone());
        let mut request = b"WDP1".to_vec();
        request.extend_from_slice(self.helper.as_bytes());
        request.extend_from_slice(nonce.as_bytes());
        // QEMU may discard writes while guestfwd reconnects. A side-effect-free
        // probe establishes readiness; it is never substituted for file data.
        let mut header = [0u8; 53];
        let mut filled = 0;
        for _ in 0..5 {
            self.write(&request).await?;
            let received = timeout(Duration::from_secs(1), async {
                while filled < header.len() {
                    let n = self.read(&mut header[filled..]).await?;
                    ensure!(n != 0, "bulk readiness channel ended");
                    filled += n;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await;
            match received {
                Err(_) => continue,
                Ok(result) => result?,
            }
            ensure!(
                &header[..4] == b"WDB1" && &header[4..40] == nonce.as_bytes(),
                "invalid bulk readiness reply"
            );
            let length = u32::from_le_bytes(header[49..53].try_into()?) as usize;
            ensure!(length <= 4096, "oversized readiness error");
            let mut error = vec![0; length];
            self.exact(&mut error).await?;
            if header[40] != 0 {
                return Err(
                    crate::qmp::GuestFailure(String::from_utf8_lossy(&error).into_owned()).into(),
                );
            }
            ensure!(
                header[41..49].iter().all(|b| *b == 0) && length == 0,
                "invalid readiness acknowledgment"
            );
            return Ok(());
        }
        anyhow::bail!("bulk worker readiness deadline exceeded")
    }
}

async fn open(runtime: &Runtime, id: &str, range: &Range, direction: u8) -> Result<Lane> {
    let mut channel = runtime.bulk_channel.clone().lock_owned().await;
    if let Some((previous, helper)) = channel.abandoned.clone() {
        if helper == range.helper_id {
            let args = json!({"transfer":previous,"helper_id":helper});
            match control(runtime, "transfer_pause", args.clone()).await {
                Ok(_) => {
                    let deadline = tokio::time::Instant::now() + IDLE;
                    loop {
                        let status = control(runtime, "transfer_status", args.clone()).await?;
                        if status["active"] == false {
                            break;
                        }
                        ensure!(
                            tokio::time::Instant::now() < deadline,
                            "abandoned range did not pause"
                        );
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                }
                Err(error) if error.downcast_ref::<crate::qmp::GuestFailure>().is_some() => {}
                Err(error) => {
                    return Err(error);
                }
            }
        }
        channel.abandoned = None;
    }
    let fresh = channel.stream.is_none();
    if fresh {
        channel.stream = Some(UnixStream::connect(runtime.run_dir.join("bulk.sock")).await?);
    }
    let mut lane = Lane {
        channel,
        cancelled: register_interrupt(runtime, id, &range.helper_id),
        nonce: range.request_id.clone().unwrap_or_else(state::id),
        ready_nonce: None,
        id: id.into(),
        helper: range.helper_id.clone(),
        reusable: false,
    };
    if fresh {
        lane.probe().await?;
    }
    let mut header = Vec::with_capacity(129);
    header.extend_from_slice(b"WDB1");
    header.extend_from_slice(range.helper_id.as_bytes());
    header.extend_from_slice(id.as_bytes());
    header.extend_from_slice(lane.nonce.as_bytes());
    header.push(direction);
    header.extend_from_slice(&range.offset.to_le_bytes());
    header.extend_from_slice(&range.length.to_le_bytes());
    lane.write(&header).await?;
    lane.reply(range.offset).await?;
    Ok(lane)
}

async fn range_check(runtime: &Runtime, id: &str, range: &Range) -> Result<()> {
    valid_id(id)?;
    if let Some(request) = &range.request_id {
        valid_id(request)?;
    }
    validate(runtime, &range.epoch, &range.helper_id).await?;
    ensure!(
        range.offset <= MAX_FILE && range.length <= MAX_FILE - range.offset,
        "invalid transfer range"
    );
    Ok(())
}

async fn upload(
    State(runtime): State<Arc<Runtime>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(range): Query<Range>,
    body: Body,
) -> Response {
    if !runtime::authorized(&runtime, &headers, false) {
        return runtime::unauthorized();
    }
    if let Err(error) = range_check(&runtime, &id, &range).await {
        return failure(error, false);
    }
    let result = async {
        let mut lane = open(&runtime, &id, &range, 1).await?;
        let mut body = body.into_data_stream();
        let mut remaining = range.length;
        let mut cancelled = lane.cancelled.clone();
        loop {
            ensure!(!*cancelled.borrow(), "bulk range interrupted");
            let chunk = tokio::select! {
                biased;
                _ = cancelled.changed() => anyhow::bail!("bulk range interrupted"),
                result = timeout(IDLE, body.next()) => result.context("upload body idle deadline")?,
            };
            let Some(chunk) = chunk else {
                break;
            };
            let chunk = chunk?;
            ensure!(
                chunk.len() as u64 <= remaining,
                "upload exceeds declared range"
            );
            for part in chunk.chunks(BUFFER) {
                lane.write(part).await?;
            }
            remaining -= chunk.len() as u64;
        }
        ensure!(remaining == 0, "upload body ended early");
        lane.reply(range.offset + range.length).await?;
        lane.reusable = true;
        Ok(json!({"offset":range.offset + range.length}))
    }
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => failure(error, true),
    }
}

async fn download(
    State(runtime): State<Arc<Runtime>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(range): Query<Range>,
) -> Response {
    if !runtime::authorized(&runtime, &headers, false) {
        return runtime::unauthorized();
    }
    if let Err(error) = range_check(&runtime, &id, &range).await {
        return failure(error, false);
    }
    let mut lane = match open(&runtime, &id, &range, 2).await {
        Ok(lane) => lane,
        Err(error) => return failure(error, true),
    };
    let length = range.length;
    if length == 0 {
        if let Err(error) = lane.reply(range.offset).await {
            return failure(error, true);
        }
        lane.reusable = true;
        return Body::empty().into_response();
    }
    let data = stream::try_unfold(
        (lane, length, range.offset + length),
        |(mut lane, remaining, end)| async move {
            if remaining == 0 {
                return Ok::<_, anyhow::Error>(None);
            }
            let mut bytes = vec![0; BUFFER.min(remaining as usize)];
            let n = lane.read(&mut bytes).await?;
            ensure!(n != 0, "download ended early");
            bytes.truncate(n);
            let next = remaining - n as u64;
            if next == 0 {
                lane.reply(end).await?;
                lane.reusable = true;
            }
            Ok(Some((bytes, (lane, next, end))))
        },
    );
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (header::CONTENT_LENGTH, length.to_string()),
        ],
        Body::from_stream(data),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interruption_belongs_to_one_correlated_range_not_a_previous_attempt() {
        let mut transfer:Transfer=serde_json::from_value(json!({"id":state::id(),"helper_id":state::id(),"path":"workspace/a","direction":"upload","size":3,"offset":1,"sha256":null,"phase":"ready","active":false,"error":"connection closed","durable":false,"range_id":"old"})).unwrap();
        let mut range = Range {
            epoch: "epoch".into(),
            helper_id: transfer.helper_id.clone(),
            offset: 1,
            length: 2,
            request_id: Some("current".into()),
        };
        assert!(!transfer.range_interrupted(&range));
        transfer.range_id = range.request_id.clone();
        assert!(transfer.range_interrupted(&range));
        transfer.active = true;
        assert!(!transfer.range_interrupted(&range));
        transfer.active = false;
        range.request_id = None;
        assert!(!transfer.range_interrupted(&range));
        let serialized = serde_json::to_value(&range).unwrap();
        assert!(serialized.get("request_id").is_none());
    }

    fn reply(nonce: &[u8], offset: u64) -> Vec<u8> {
        let mut bytes = b"WDB1".to_vec();
        bytes.extend_from_slice(nonce);
        bytes.push(0);
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes
    }

    #[tokio::test]
    async fn binary_ranges_stream_large_payloads_and_reuse_the_lane_without_control_lock() {
        let dir = tempfile::tempdir().unwrap();
        let helper = state::id();
        let id = state::id();
        let runtime = Runtime::fixture(
            dir.path(),
            json!({"guest":{"helper_id":helper,"interactive":true,"features":{"binary_files":1}}}),
        );
        let unix = tokio::net::UnixListener::bind(dir.path().join("bulk.sock")).unwrap();
        let payload = vec![0xa5; 1024 * 1024 + 17];
        let expected = payload.clone();
        let worker = tokio::spawn(async move {
            let (mut channel, _) = unix.accept().await.unwrap();
            let mut probe = [0u8; 76];
            channel.read_exact(&mut probe).await.unwrap();
            assert_eq!(&probe[..4], b"WDP1");
            channel.write_all(&reply(&probe[40..76], 0)).await.unwrap();
            for direction in [1u8, 2] {
                let mut header = [0u8; 129];
                channel.read_exact(&mut header).await.unwrap();
                assert_eq!(&header[..4], b"WDB1");
                assert_eq!(header[112], direction);
                assert_eq!(
                    u64::from_le_bytes(header[121..129].try_into().unwrap()),
                    expected.len() as u64
                );
                channel
                    .write_all(&reply(&header[76..112], 0))
                    .await
                    .unwrap();
                if direction == 1 {
                    let mut received = vec![0; expected.len()];
                    channel.read_exact(&mut received).await.unwrap();
                    assert_eq!(received, expected);
                } else {
                    channel.write_all(&expected).await.unwrap();
                }
                channel
                    .write_all(&reply(&header[76..112], expected.len() as u64))
                    .await
                    .unwrap();
            }
        });
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let app = runtime::router(runtime.clone());
        let server = tokio::spawn(async move { axum::serve(tcp, app).await.unwrap() });
        let control = runtime.guest_lock.lock().await; // Bulk must not acquire this.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let url = format!("http://{address}/v1/transfers/{id}/data");
        let range = Range {
            epoch: "test-epoch".into(),
            helper_id: helper,
            offset: 0,
            length: payload.len() as u64,
            request_id: None,
        };
        let response = http
            .put(&url)
            .query(&range)
            .bearer_auth("data-secret")
            .body(payload.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await.unwrap()["offset"],
            payload.len()
        );
        assert!(runtime.bulk_channel.lock().await.stream.is_some());
        let response = http
            .get(&url)
            .query(&range)
            .bearer_auth("data-secret")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.bytes().await.unwrap().as_ref(), payload);
        assert!(runtime.bulk_channel.lock().await.stream.is_some());
        worker.await.unwrap();
        drop(control);
        server.abort();
    }

    #[tokio::test]
    async fn bulk_authority_and_epochs_are_checked_before_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let helper = state::id();
        let id = state::id();
        let runtime = Runtime::fixture(
            dir.path(),
            json!({"guest":{"helper_id":helper,"features":{"binary_files":1}}}),
        );
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let app = runtime::router(runtime.clone());
        let server = tokio::spawn(async move { axum::serve(tcp, app).await.unwrap() });
        let http = reqwest::Client::new();
        let range = Range {
            epoch: "old-epoch".into(),
            helper_id: helper,
            offset: 0,
            length: 3,
            request_id: None,
        };
        let url = format!("http://{address}/v1/transfers/{id}/data");
        for token in ["viewer-secret", "wrong"] {
            assert_eq!(
                http.put(&url)
                    .query(&range)
                    .bearer_auth(token)
                    .body("abc")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                http.get(&url)
                    .query(&range)
                    .bearer_auth(token)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let response = http
            .put(&url)
            .query(&range)
            .bearer_auth("data-secret")
            .body("abc")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"]["outcome"],
            "not_started"
        );
        assert!(runtime.bulk_channel.lock().await.stream.is_none());
        server.abort();
    }

    #[tokio::test]
    async fn stale_reply_poisoned_lane_is_not_reused() {
        let (host, mut peer) = UnixStream::pair().unwrap();
        let channel = Arc::new(tokio::sync::Mutex::new(Channel {
            stream: Some(host),
            abandoned: None,
        }));
        let nonce = state::id();
        let (_signal, cancelled) = watch::channel(false);
        let mut lane = Lane {
            channel: channel.clone().lock_owned().await,
            cancelled,
            nonce: nonce.clone(),
            ready_nonce: None,
            id: state::id(),
            helper: state::id(),
            reusable: false,
        };
        peer.write_all(&reply(state::id().as_bytes(), 7))
            .await
            .unwrap();
        assert!(
            lane.reply(7)
                .await
                .unwrap_err()
                .to_string()
                .contains("stale")
        );
        drop(lane);
        assert!(channel.lock().await.stream.is_none());
    }

    #[tokio::test]
    async fn interruption_releases_an_idle_upload_body_without_waiting_for_its_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let helper = state::id();
        let id = state::id();
        let runtime = Runtime::fixture(
            dir.path(),
            json!({"guest":{"helper_id":helper,"features":{"binary_files":1}}}),
        );
        let unix = tokio::net::UnixListener::bind(dir.path().join("bulk.sock")).unwrap();
        let (ready, waiting) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            let (mut channel, _) = unix.accept().await.unwrap();
            let mut probe = [0; 76];
            channel.read_exact(&mut probe).await.unwrap();
            channel.write_all(&reply(&probe[40..76], 0)).await.unwrap();
            let mut header = [0; 129];
            channel.read_exact(&mut header).await.unwrap();
            channel
                .write_all(&reply(&header[76..112], 0))
                .await
                .unwrap();
            ready.send(()).unwrap();
            match channel.read(&mut [0; 1]).await {
                Ok(0) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                result => panic!("expected interrupted channel, got {result:?}"),
            }
        });
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer data-secret".parse().unwrap());
        let range = Range {
            epoch: "test-epoch".into(),
            helper_id: helper.clone(),
            offset: 0,
            length: 3,
            request_id: Some(state::id()),
        };
        let task = tokio::spawn(upload(
            State(runtime.clone()),
            headers,
            Path(id.clone()),
            Query(range),
            Body::from_stream(stream::pending::<Result<axum::body::Bytes, std::io::Error>>()),
        ));
        waiting.await.unwrap();
        interrupt(&runtime, &state::id(), &helper);
        interrupt(&runtime, &id, &state::id());
        assert!(!task.is_finished());
        interrupt(&runtime, &id, &helper);
        let response = timeout(Duration::from_millis(200), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        worker.await.unwrap();
        let channel = runtime.bulk_channel.lock().await;
        assert!(channel.stream.is_none());
        assert_eq!(channel.abandoned, Some((id, helper)));
    }
    #[test]
    fn transfer_ids_are_canonical_and_metadata_is_strict() {
        assert!(valid_id("01234567-89ab-cdef-0123-456789abcdef").is_ok());
        assert!(valid_id("0123456789abcdef0123456789abcdef").is_err());
        assert!(
            serde_json::from_value::<Identity>(
                json!({"epoch":"a","helper_id":"b","token":"secret"})
            )
            .is_err()
        );
    }
}
