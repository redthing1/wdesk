use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::{Duration, timeout},
};

pub struct Qmp {
    path: PathBuf,
}
impl Qmp {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub async fn command(&self, execute: &str, arguments: Value) -> Result<Value> {
        timeout(Duration::from_secs(10), self.exchange(execute, arguments))
            .await
            .context("QMP deadline exceeded")?
    }
    async fn exchange(&self, execute: &str, arguments: Value) -> Result<Value> {
        let socket = UnixStream::connect(&self.path)
            .await
            .context("QEMU not responding")?;
        let mut stream = BufReader::new(socket);
        let mut line = String::new();
        ensure!(stream.read_line(&mut line).await? > 0, "QMP closed");
        let greeting: Value = serde_json::from_str(&line)?;
        ensure!(greeting.get("QMP").is_some(), "invalid QMP greeting");
        for (seq, cmd) in [
            json!({"execute":"qmp_capabilities","id":1}),
            json!({"execute":execute,"arguments":arguments,"id":2}),
        ]
        .iter()
        .enumerate()
        {
            stream
                .get_mut()
                .write_all(format!("{cmd}\n").as_bytes())
                .await?;
            loop {
                line.clear();
                ensure!(stream.read_line(&mut line).await? > 0, "QMP closed");
                ensure!(line.len() <= 1024 * 1024, "oversized QMP response");
                let reply: Value = serde_json::from_str(&line)?;
                if reply.get("id") != Some(&json!(seq + 1)) {
                    continue;
                }
                if let Some(error) = reply.get("error") {
                    bail!("QMP {execute}: {error}");
                }
                if seq == 1 {
                    return Ok(reply["return"].clone());
                }
                break;
            }
        }
        unreachable!()
    }
    pub async fn screenshot(&self, path: &Path) -> Result<Vec<u8>> {
        self.command("screendump", json!({"filename":path,"format":"png"}))
            .await?;
        let bytes = tokio::fs::read(path).await?;
        let _ = tokio::fs::remove_file(path).await;
        Ok(bytes)
    }
    pub async fn events(&self, events: Vec<Value>) -> Result<()> {
        if !events.is_empty() {
            self.command("input-send-event", json!({"events":events}))
                .await?;
        }
        Ok(())
    }
}

pub fn key_event(code: &str, down: bool) -> Value {
    json!({"type":"key","data":{"down":down,"key":{"type":"qcode","data":code}}})
}

pub fn ascii_key(c: char) -> Result<(String, bool)> {
    if c.is_ascii_alphanumeric() {
        return Ok((c.to_ascii_lowercase().to_string(), c.is_ascii_uppercase()));
    }
    let (code, shift) = match c {
        ' ' => ("spc", false),
        '\n' | '\r' => ("ret", false),
        '\t' => ("tab", false),
        '-' => ("minus", false),
        '_' => ("minus", true),
        '=' => ("equal", false),
        '+' => ("equal", true),
        '[' => ("bracket_left", false),
        '{' => ("bracket_left", true),
        ']' => ("bracket_right", false),
        '}' => ("bracket_right", true),
        ';' => ("semicolon", false),
        ':' => ("semicolon", true),
        '\'' => ("apostrophe", false),
        '"' => ("apostrophe", true),
        '\\' => ("backslash", false),
        '|' => ("backslash", true),
        ',' => ("comma", false),
        '<' => ("comma", true),
        '.' => ("dot", false),
        '>' => ("dot", true),
        '/' => ("slash", false),
        '?' => ("slash", true),
        '`' => ("grave_accent", false),
        '~' => ("grave_accent", true),
        '!' => ("1", true),
        '@' => ("2", true),
        '#' => ("3", true),
        '$' => ("4", true),
        '%' => ("5", true),
        '^' => ("6", true),
        '&' => ("7", true),
        '*' => ("8", true),
        '(' => ("9", true),
        ')' => ("0", true),
        _ => bail!("unsupported console character"),
    };
    Ok((code.into(), shift))
}
pub fn button_event(button: &str, down: bool) -> Value {
    json!({"type":"btn","data":{"button":button,"down":down}})
}
pub fn move_events(x: u32, y: u32, w: u32, h: u32) -> Vec<Value> {
    vec![
        json!({"type":"abs","data":{"axis":"x","value":(u64::from(x)*32767/u64::from(w.saturating_sub(1).max(1)))}}),
        json!({"type":"abs","data":{"axis":"y","value":(u64::from(y)*32767/u64::from(h.saturating_sub(1).max(1)))}}),
    ]
}

pub async fn guest(path: &Path, request: Value) -> Result<Value> {
    GuestChannel::new(path.to_path_buf()).request(request).await
}

pub struct GuestChannel {
    path: PathBuf,
    stream: Option<BufReader<UnixStream>>,
}
impl GuestChannel {
    pub fn new(path: PathBuf) -> Self {
        Self { path, stream: None }
    }
    pub fn disconnect(&mut self) {
        self.stream = None;
    }
    pub async fn request(&mut self, request: Value) -> Result<Value> {
        let reply = timeout(Duration::from_secs(25), self.exchange(request)).await;
        let reply = match reply {
            Ok(Ok(reply)) => reply,
            Ok(Err(error)) => {
                self.disconnect();
                return Err(error);
            }
            Err(error) => {
                self.disconnect();
                return Err(error)
                    .context("guest helper deadline exceeded; guest may be booting or locked");
            }
        };
        if reply["ok"] == true {
            return Ok(reply["result"].clone());
        }
        bail!(
            "guest: {}",
            reply["error"].as_str().unwrap_or("operation failed")
        );
    }
    async fn exchange(&mut self, request: Value) -> Result<Value> {
        if self.stream.is_none() {
            self.stream = Some(BufReader::new(
                UnixStream::connect(&self.path)
                    .await
                    .context("guest channel unavailable")?,
            ));
        }
        let stream = self.stream.as_mut().expect("connected guest channel");
        let request_id = uuid::Uuid::new_v4().to_string();
        let mut request = request;
        request["id"] = json!(request_id);
        stream
            .get_mut()
            .write_all(format!("{request}\n").as_bytes())
            .await?;
        // On reconnect a timed-out request can leave a stale reply. Correlate ids.
        loop {
            let mut response = Vec::new();
            use tokio::io::AsyncReadExt;
            let n = (&mut *stream)
                .take(2 * 1024 * 1024)
                .read_until(b'\n', &mut response)
                .await?;
            ensure!(
                n > 0 && response.last() == Some(&b'\n'),
                "guest closed or oversized response"
            );
            let response: Value = serde_json::from_slice(&response)?;
            if response["id"] != request_id {
                continue;
            }
            return Ok(response);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_coordinates_map_to_full_tablet_range() {
        let e = move_events(1023, 767, 1024, 768);
        assert_eq!(e[0]["data"]["value"], 32767);
        assert_eq!(e[1]["data"]["value"], 32767);
        assert_eq!(move_events(0, 0, 1, 1)[0]["data"]["value"], 0);
    }
}
