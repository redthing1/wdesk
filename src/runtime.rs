use crate::{
    protocol::{Action, Batch, GuestOp, GuestRequest},
    qmp::{self, Qmp},
    state::{self, Descriptor, VmConfig},
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use subtle::ConstantTimeEq;
use tokio::{
    process::Command,
    sync::{Mutex, RwLock},
    time::{Duration, sleep},
};

pub struct Runtime {
    pub descriptor: Descriptor,
    pub viewer_token: String,
    pub run_dir: PathBuf,
    pub qmp: Qmp,
    pub input: Mutex<InputState>,
    pub guest_lock: Mutex<()>,
    pub fast_ready: AtomicBool,
    pub fast_channel: Mutex<qmp::GuestChannel>,
    pub health: RwLock<Value>,
}

pub struct InputState {
    generation: u64,
    observation: u64,
    width: u32,
    height: u32,
    guest_epoch: Option<String>,
    cache: VecDeque<(String, String, Value)>,
}

impl InputState {
    fn note_guest(&mut self, next: &str) {
        if self.guest_epoch.as_deref().is_some_and(|old| old != next) {
            self.generation += 1;
        }
        self.guest_epoch = Some(next.to_owned());
    }
}

pub fn vm_args(state_dir: &Path, run_dir: &Path, config: &VmConfig) -> Result<Vec<String>> {
    ensure!(
        (512..=131072).contains(&config.memory_mb) && (1..=64).contains(&config.cpus),
        "invalid VM resources"
    );
    // QEMU comma-separated argument grammar needs paths without commas.
    for path in [state_dir, run_dir]
        .into_iter()
        .chain(config.install_iso.as_deref())
        .chain(config.seed_iso.as_deref())
        .chain(config.answer_disk.as_deref())
    {
        ensure!(
            !path.as_os_str().as_encoded_bytes().contains(&b','),
            "QEMU paths cannot contain commas"
        );
    }
    let mut args = vec![
        "-name".into(),
        "wdesk".into(),
        "-machine".into(),
        format!("q35,accel={}", if config.tcg { "tcg" } else { "kvm" }),
        "-cpu".into(),
        if config.tcg {
            "max".into()
        } else {
            "host".into()
        },
        "-m".into(),
        config.memory_mb.to_string(),
        "-smp".into(),
        config.cpus.to_string(),
        "-display".into(),
        "none".into(),
        "-vga".into(),
        "std".into(),
        "-device".into(),
        "qemu-xhci".into(),
        "-device".into(),
        "usb-tablet".into(),
        "-qmp".into(),
        format!("unix:{}/qmp.sock,server=on,wait=off", run_dir.display()),
        "-chardev".into(),
        format!(
            "socket,id=guest,path={}/guest.sock,server=on,wait=off",
            run_dir.display()
        ),
        "-device".into(),
        "isa-serial,chardev=guest".into(),
        "-drive".into(),
        format!(
            "file={}/system.qcow2,if=ide,index=0,format=qcow2,discard=unmap",
            state_dir.display()
        ),
        "-chardev".into(),
        format!(
            "socket,id=fast,path={}/fast.sock,server=on,wait=off",
            run_dir.display()
        ),
        "-netdev".into(),
        format!(
            "user,id=net0,restrict={},ipv6=off,guestfwd=tcp:10.0.2.100:9843-chardev:fast",
            if config.offline { "on" } else { "off" }
        ),
        "-device".into(),
        "e1000e,netdev=net0".into(),
    ];
    if let Some(iso) = &config.install_iso {
        args.extend([
            "-drive".into(),
            format!(
                "file={},media=cdrom,if=ide,index=2,readonly=on",
                iso.display()
            ),
            "-boot".into(),
            "once=d,order=c,menu=off".into(),
        ]);
    }
    if let Some(seed) = &config.seed_iso {
        args.extend([
            "-drive".into(),
            format!(
                "file={},media=cdrom,if=ide,index=3,readonly=on",
                seed.display()
            ),
        ]);
    }
    if let Some(answer) = &config.answer_disk {
        args.extend([
            "-drive".into(),
            format!("file={},if=floppy,format=raw,readonly=on", answer.display()),
        ]);
    }
    Ok(args)
}

pub async fn serve(state_dir: PathBuf, bind: String) -> Result<()> {
    state::private_dir(&state_dir)?;
    let _lock = state::lock(&state_dir.join("runtime.lock"))?;
    let config: VmConfig = state::read_json(&state_dir.join("vm.json"))?;
    let run_dir = tempfile::Builder::new()
        .prefix("wdesk-")
        .tempdir_in("/tmp")?;
    let qemu_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir.join("qemu.log"))?;
    let mut qemu = Command::new("qemu-system-x86_64")
        .args(vm_args(&state_dir, run_dir.path(), &config)?)
        .stdin(std::process::Stdio::null())
        .stdout(qemu_log.try_clone()?)
        .stderr(qemu_log)
        .kill_on_drop(true)
        .spawn()
        .context("starting QEMU; install qemu-system-x86")?;
    let qmp = Qmp::new(run_dir.path().join("qmp.sock"));
    let mut started = false;
    for _ in 0..100 {
        if let Some(status) = qemu.try_wait()? {
            anyhow::bail!(
                "QEMU exited {status}; see {}",
                state_dir.join("qemu.log").display()
            );
        }
        if qmp.command("query-status", json!({})).await.is_ok() {
            started = true;
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    ensure!(started, "QEMU startup deadline exceeded");
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let address = listener.local_addr()?;
    let descriptor = Descriptor {
        protocol: 1,
        endpoint: format!("http://127.0.0.1:{}", address.port()),
        token: state::secret(),
        epoch: state::id(),
    };
    let viewer_token = state::secret();
    state::write_json(&state_dir.join("runtime-client.json"), &descriptor)?;
    state::write_json(
        &state_dir.join("runtime-viewer.json"),
        &json!({"token":viewer_token}),
    )?;
    let runtime = Arc::new(Runtime {
        descriptor,
        viewer_token,
        run_dir: run_dir.path().into(),
        qmp,
        input: Mutex::new(InputState {
            generation: 0,
            observation: 0,
            width: 0,
            height: 0,
            guest_epoch: None,
            cache: VecDeque::new(),
        }),
        guest_lock: Mutex::new(()),
        fast_ready: AtomicBool::new(false),
        fast_channel: Mutex::new(qmp::GuestChannel::new(run_dir.path().join("fast.sock"))),
        health: RwLock::new(json!({"state":"vm_running","guest":null})),
    });
    let monitor = tokio::spawn(monitor(runtime.clone()));
    let app = router(runtime);
    let server = std::future::IntoFuture::into_future(
        axum::serve(listener, app).with_graceful_shutdown(async {
            shutdown_signal().await;
        }),
    );
    tokio::pin!(server);
    // Use an explicit IntoFuture because axum's graceful server implements it.
    let result = tokio::select! {
        result=&mut server => result.context("HTTP server"),
        result=qemu.wait()=> { anyhow::bail!("QEMU exited unexpectedly: {:?}",result?); }
    };
    monitor.abort();
    let _ = qmp_shutdown(run_dir.path()).await;
    for _ in 0..100 {
        if qemu.try_wait()?.is_some() {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    if qemu.try_wait()?.is_none() {
        let _ = qemu.kill().await;
    }
    let _ = qemu.wait().await;
    let _ = std::fs::remove_file(state_dir.join("runtime-client.json"));
    let _ = std::fs::remove_file(state_dir.join("runtime-viewer.json"));
    result
}

async fn qmp_shutdown(run_dir: &Path) -> Result<()> {
    Qmp::new(run_dir.join("qmp.sock"))
        .command("system_powerdown", json!({}))
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
}

async fn monitor(runtime: Arc<Runtime>) {
    loop {
        let result = {
            let _guard = runtime.guest_lock.lock().await;
            let mut fast_channel = runtime.fast_channel.lock().await;
            let fast = tokio::time::timeout(
                Duration::from_millis(700),
                fast_channel.request(json!({"op":"health","args":{}})),
            )
            .await;
            if let Ok(Ok(value)) = fast {
                runtime.fast_ready.store(true, Ordering::Relaxed);
                Ok(Ok(value))
            } else {
                fast_channel.disconnect();
                runtime.fast_ready.store(false, Ordering::Relaxed);
                tokio::time::timeout(
                    Duration::from_secs(3),
                    qmp::guest(
                        &runtime.run_dir.join("guest.sock"),
                        json!({"op":"health","args":{}}),
                    ),
                )
                .await
            }
        };
        let value = match result {
            Ok(Ok(guest)) if guest["interactive"] == true => {
                json!({"state":"helper_ready","guest":guest})
            }
            Ok(Ok(guest)) => json!({"state":"guest_responding","guest":guest}),
            _ => json!({"state":"vm_running","guest":null}),
        };
        if let Some(next) = value["guest"]["helper_id"].as_str() {
            runtime.input.lock().await.note_guest(next);
        }
        *runtime.health.write().await = value;
        sleep(Duration::from_secs(5)).await;
    }
}

impl Runtime {
    async fn guest_request(&self, request: Value) -> Result<Value> {
        let fast = self.fast_ready.load(Ordering::Relaxed);
        let response = if fast {
            self.fast_channel.lock().await.request(request).await
        } else {
            qmp::guest(&self.run_dir.join("guest.sock"), request).await
        };
        if response.is_err() && fast {
            self.fast_ready.store(false, Ordering::Relaxed);
        }
        // A failed mutation may already have run. Never retry on another transport.
        response
    }
}

pub fn router(runtime: Arc<Runtime>) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/see", get(see))
        .route("/v1/batch", post(batch))
        .route("/v1/guest", post(guest))
        .route("/viewer", get(viewer))
        .route("/viewer.js", get(viewer_js))
        .layer(axum::extract::DefaultBodyLimit::max(128 * 1024))
        .with_state(runtime)
}

fn authorized(runtime: &Runtime, headers: &HeaderMap, allow_viewer: bool) -> bool {
    let Some(token) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    token
        .as_bytes()
        .ct_eq(runtime.descriptor.token.as_bytes())
        .into()
        || (allow_viewer && bool::from(token.as_bytes().ct_eq(runtime.viewer_token.as_bytes())))
}
fn error(status: StatusCode, code: &str, message: impl ToString) -> Response {
    (status,Json(json!({"ok":false,"error":{"code":code,"message":message.to_string(),"retryable":status==StatusCode::SERVICE_UNAVAILABLE}}))).into_response()
}
fn unauthorized() -> Response {
    error(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "session credential required",
    )
}

async fn health(State(runtime): State<Arc<Runtime>>, headers: HeaderMap) -> Response {
    if !authorized(&runtime, &headers, true) {
        return unauthorized();
    }
    let input = runtime.input.lock().await;
    let mut result = runtime.health.read().await.clone();
    if result["state"] == "helper_ready" && input.width > 0 {
        result["state"] = json!("automation_ready");
    }
    result["epoch"] = json!(runtime.descriptor.epoch);
    result["input_generation"] = json!(input.generation);
    result["geometry"] = json!({"width":input.width,"height":input.height});
    Json(result).into_response()
}

async fn capabilities(State(runtime): State<Arc<Runtime>>, headers: HeaderMap) -> Response {
    if !authorized(&runtime, &headers, false) {
        return unauthorized();
    }
    let ready = runtime.health.read().await["guest"]["interactive"] == true;
    Json(json!({"protocol":1,"epoch":runtime.descriptor.epoch,"console":{"screenshots":true,"physical_input":true,"cursor_included":false},"guest":{"ready":ready,"transport":if runtime.fast_ready.load(Ordering::Relaxed){"tcp_guestfwd"}else{"serial"},"windows":ready,"clipboard":ready,"processes":ready,"files":ready,"a11y":ready},"limits":{"batch_actions":64,"file_bytes":crate::protocol::MAX_FILE,"chunk_bytes":crate::protocol::CHUNK,"process_output_characters":65536},"input_semantics":"serialized batches; delivery is not application success","viewer":"shared canvas; input serialized with agents"})).into_response()
}

async fn capture(runtime: &Runtime, input: &mut InputState) -> Result<(Value, Vec<u8>)> {
    let bytes = runtime
        .qmp
        .screenshot(&runtime.run_dir.join("screen.png"))
        .await?;
    let image = image::load_from_memory(&bytes)?;
    input.width = image.width();
    input.height = image.height();
    input.observation += 1;
    let guest_epoch = runtime.health.read().await["guest"]["helper_id"].clone();
    let meta = json!({"protocol":1,"epoch":runtime.descriptor.epoch,"guest_epoch":guest_epoch,"observation_id":input.observation,"input_generation":input.generation,"geometry":{"width":input.width,"height":input.height},"sha256":hex::encode(Sha256::digest(&bytes)),"bytes":bytes.len(),"coordinate_space":"native_framebuffer_pixels","cursor_included":false});
    Ok((meta, bytes))
}
async fn see(State(runtime): State<Arc<Runtime>>, headers: HeaderMap) -> Response {
    if !authorized(&runtime, &headers, true) {
        return unauthorized();
    }
    let mut input = runtime.input.lock().await;
    match capture(&runtime, &mut input).await {
        Ok((meta, bytes)) => (
            [
                ("content-type", "image/png"),
                ("cache-control", "no-store"),
                ("x-wdesk-observation", &meta.to_string()),
            ],
            Bytes::from(bytes),
        )
            .into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, "capture_unavailable", e),
    }
}

async fn batch(
    State(runtime): State<Arc<Runtime>>,
    headers: HeaderMap,
    Json(batch): Json<Batch>,
) -> Response {
    if !authorized(&runtime, &headers, true) {
        return unauthorized();
    }
    let mut input = runtime.input.lock().await;
    if batch.epoch != runtime.descriptor.epoch {
        return error(
            StatusCode::CONFLICT,
            "stale_epoch",
            "runtime restarted; observe again",
        );
    }
    if input.width == 0
        && let Err(e) = capture(&runtime, &mut input).await
    {
        return error(StatusCode::SERVICE_UNAVAILABLE, "capture_unavailable", e);
    }
    if let Err(e) = batch.validate(input.width, input.height) {
        return error(StatusCode::BAD_REQUEST, "invalid_batch", e);
    }
    let fingerprint = hex::encode(Sha256::digest(
        serde_json::to_vec(&batch).expect("serialize batch"),
    ));
    if let Some((_, previous, result)) = input
        .cache
        .iter()
        .find(|(id, _, _)| id == &batch.request_id)
    {
        if previous != &fingerprint {
            return error(
                StatusCode::CONFLICT,
                "request_id_reused",
                "request_id already used with different content",
            );
        }
        return Json(result.clone()).into_response();
    }
    if batch
        .expected_input_generation
        .is_some_and(|g| g != input.generation)
    {
        return error(
            StatusCode::CONFLICT,
            "stale_input",
            "input changed; observe again",
        );
    }
    let before = input.generation;
    let mut results = vec![];
    for action in &batch.actions {
        if !matches!(action, Action::Wait { .. }) {
            input.generation += 1;
        }
        let result = deliver(&runtime, action, input.width, input.height).await;
        match result {
            Ok(()) => results.push(json!({"delivered":true})),
            Err(e) => {
                results.push(
                    json!({"delivered":false,"delivery_uncertain":true,"error":e.to_string()}),
                );
                break;
            }
        }
    }
    let result = json!({"protocol":1,"epoch":runtime.descriptor.epoch,"request_id":batch.request_id,"input_generation_before":before,"input_generation":input.generation,"completed":results.iter().filter(|r|r["delivered"]==true).count(),"results":results,"application_success":null});
    input
        .cache
        .push_back((batch.request_id, fingerprint, result.clone()));
    if input.cache.len() > 256 {
        input.cache.pop_front();
    }
    Json(result).into_response()
}

async fn deliver(runtime: &Runtime, action: &Action, w: u32, h: u32) -> Result<()> {
    match action {
        Action::Wait { milliseconds } => sleep(Duration::from_millis(*milliseconds)).await,
        Action::Move { x, y } => runtime.qmp.events(qmp::move_events(*x, *y, w, h)).await?,
        Action::Click { x, y, button } => {
            runtime.qmp.events(qmp::move_events(*x, *y, w, h)).await?;
            let result = runtime
                .qmp
                .events(vec![qmp::button_event(button, true)])
                .await;
            let release = runtime
                .qmp
                .events(vec![qmp::button_event(button, false)])
                .await;
            result?;
            release?;
        }
        Action::Drag { x, y, to_x, to_y } => {
            runtime.qmp.events(qmp::move_events(*x, *y, w, h)).await?;
            let result = async {
                runtime
                    .qmp
                    .events(vec![qmp::button_event("left", true)])
                    .await?;
                for step in 1..=10 {
                    let x = (i64::from(*x) + (i64::from(*to_x) - i64::from(*x)) * step / 10) as u32;
                    let y = (i64::from(*y) + (i64::from(*to_y) - i64::from(*y)) * step / 10) as u32;
                    runtime.qmp.events(qmp::move_events(x, y, w, h)).await?;
                    sleep(Duration::from_millis(20)).await;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            let release = runtime
                .qmp
                .events(vec![qmp::button_event("left", false)])
                .await;
            result?;
            release?;
        }
        Action::Scroll { direction, steps } => {
            let button = format!("wheel-{direction}");
            for _ in 0..*steps {
                runtime
                    .qmp
                    .events(vec![qmp::button_event(&button, true)])
                    .await?;
                runtime
                    .qmp
                    .events(vec![qmp::button_event(&button, false)])
                    .await?;
            }
        }
        Action::Key { keys } => {
            let codes = keys
                .iter()
                .map(|k| crate::protocol::qcode(k))
                .collect::<Result<Vec<_>>>()?;
            let result = runtime
                .qmp
                .events(codes.iter().map(|k| qmp::key_event(k, true)).collect())
                .await;
            sleep(Duration::from_millis(40)).await;
            let release = runtime
                .qmp
                .events(
                    codes
                        .iter()
                        .rev()
                        .map(|k| qmp::key_event(k, false))
                        .collect(),
                )
                .await;
            result?;
            release?;
        }
        Action::TypeText { text } => {
            let _guard = runtime.guest_lock.lock().await;
            runtime
                .guest_request(json!({"op":"type_text","args":{"text":text}}))
                .await?;
        }
        Action::TypeAscii { text } => {
            for c in text.chars() {
                let (code, shift) = qmp::ascii_key(c)?;
                let mut down = vec![];
                if shift {
                    down.push(qmp::key_event("shift", true));
                }
                down.push(qmp::key_event(&code, true));
                let result = runtime.qmp.events(down).await;
                sleep(Duration::from_millis(4)).await;
                let mut up = vec![qmp::key_event(&code, false)];
                if shift {
                    up.push(qmp::key_event("shift", false));
                }
                let release = runtime.qmp.events(up).await;
                result?;
                release?;
            }
        }
    }
    Ok(())
}

async fn guest(
    State(runtime): State<Arc<Runtime>>,
    headers: HeaderMap,
    Json(request): Json<GuestRequest>,
) -> Response {
    if !authorized(&runtime, &headers, false) {
        return unauthorized();
    }
    let _input = if matches!(
        request.op,
        GuestOp::Focus | GuestOp::TypeText | GuestOp::Launch | GuestOp::ProcessStart
    ) {
        let mut guard = runtime.input.lock().await;
        guard.generation += 1;
        Some(guard)
    } else {
        None
    };
    let _guard = runtime.guest_lock.lock().await;
    match runtime
        .guest_request(serde_json::to_value(request).expect("guest request"))
        .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, "guest_operation_failed", e),
    }
}

async fn viewer() -> Response {
    asset(
        "text/html; charset=utf-8",
        include_str!("../assets/viewer.html"),
    )
}
async fn viewer_js() -> Response {
    asset(
        "text/javascript; charset=utf-8",
        include_str!("../assets/viewer.js"),
    )
}
fn asset(kind: &str, body: &'static str) -> Response {
    ([("content-type",kind),("cache-control","no-store"),("referrer-policy","no-referrer"),("content-security-policy","default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; img-src 'self' blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'")],body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_incarnation_survives_unavailable_health() {
        let mut input = InputState {
            generation: 0,
            observation: 0,
            width: 0,
            height: 0,
            guest_epoch: None,
            cache: VecDeque::new(),
        };
        input.note_guest("first");
        input.note_guest("first");
        assert_eq!(input.generation, 0);
        // Unavailable health does not clear the remembered incarnation.
        input.note_guest("second");
        assert_eq!(input.generation, 1);
        input.note_guest("second");
        assert_eq!(input.generation, 1);
    }
    #[test]
    fn qemu_has_no_published_control_or_shares() {
        let args = vm_args(
            Path::new("/state"),
            Path::new("/tmp/wdesk-a"),
            &VmConfig::default(),
        )
        .unwrap()
        .join(" ");
        assert!(!args.contains("hostfwd"));
        assert!(!args.contains("-vnc"));
        assert!(!args.contains("-virtfs"));
        assert!(args.contains("usb-tablet"));
        assert!(args.contains("accel=kvm"));
        assert!(
            vm_args(
                Path::new("/state,evil"),
                Path::new("/tmp/a"),
                &VmConfig::default()
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn viewer_authority_cannot_call_guest_or_get_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(Runtime {
            descriptor: Descriptor {
                protocol: 1,
                endpoint: "http://127.0.0.1:1".into(),
                token: "data-secret".into(),
                epoch: "epoch".into(),
            },
            viewer_token: "viewer-secret".into(),
            run_dir: dir.path().into(),
            qmp: Qmp::new(dir.path().join("missing")),
            input: Mutex::new(InputState {
                generation: 0,
                observation: 0,
                width: 1,
                height: 1,
                guest_epoch: None,
                cache: VecDeque::new(),
            }),
            guest_lock: Mutex::new(()),
            fast_ready: AtomicBool::new(false),
            fast_channel: Mutex::new(qmp::GuestChannel::new(dir.path().join("missing-fast"))),
            health: RwLock::new(json!({"state":"vm_running","guest":null})),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server =
            tokio::spawn(async move { axum::serve(listener, router(runtime)).await.unwrap() });
        let http = reqwest::Client::new();
        let base = format!("http://{address}");
        assert_eq!(
            http.get(format!("{base}/v1/health"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            http.get(format!("{base}/v1/health"))
                .bearer_auth("viewer-secret")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            http.get(format!("{base}/v1/capabilities"))
                .bearer_auth("viewer-secret")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            http.post(format!("{base}/v1/guest"))
                .bearer_auth("viewer-secret")
                .json(&json!({"op":"health","args":{}}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let response=http.post(format!("{base}/v1/batch")).bearer_auth("data-secret").json(&json!({"protocol":1,"request_id":"r","epoch":"old","actions":[{"type":"key","keys":["S"]}]})).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"]["code"],
            "stale_epoch"
        );
        server.abort();
    }
}
