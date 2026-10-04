use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL: u32 = 1;
pub const CHUNK: usize = 48 * 1024;
pub const MAX_FILE: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationOutcome {
    NotStarted,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OperationError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub outcome: OperationOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub protocol: u32,
    pub request_id: String,
    pub epoch: String,
    pub expected_input_generation: Option<u64>,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Move {
        x: u32,
        y: u32,
    },
    Click {
        x: u32,
        y: u32,
        #[serde(default = "left")]
        button: String,
    },
    Drag {
        x: u32,
        y: u32,
        to_x: u32,
        to_y: u32,
    },
    Scroll {
        direction: String,
        steps: u32,
    },
    Key {
        keys: Vec<String>,
    },
    TypeText {
        text: String,
    },
    TypeAscii {
        text: String,
    },
    Wait {
        milliseconds: u64,
    },
}

fn left() -> String {
    "left".into()
}

impl Batch {
    pub fn validate(&self, width: u32, height: u32) -> Result<()> {
        ensure!(self.protocol == PROTOCOL, "unsupported protocol");
        ensure!(
            !self.request_id.is_empty() && self.request_id.len() <= 128,
            "invalid request_id"
        );
        ensure!(
            !self.actions.is_empty() && self.actions.len() <= 64,
            "batch needs 1..64 actions"
        );
        let mut wait = 0;
        let mut unicode_bytes = 0usize;
        let mut ascii_bytes = 0usize;
        for action in &self.actions {
            let coord = |x, y| -> Result<()> {
                ensure!(x < width && y < height, "coordinates outside framebuffer");
                Ok(())
            };
            match action {
                Action::Move { x, y } => coord(*x, *y)?,
                Action::Click { x, y, button } => {
                    coord(*x, *y)?;
                    ensure!(
                        ["left", "middle", "right"].contains(&button.as_str()),
                        "invalid mouse button"
                    );
                }
                Action::Drag { x, y, to_x, to_y } => {
                    coord(*x, *y)?;
                    coord(*to_x, *to_y)?;
                }
                Action::Scroll { direction, steps } => {
                    ensure!(
                        ["up", "down", "left", "right"].contains(&direction.as_str())
                            && *steps <= 100,
                        "invalid scroll"
                    );
                }
                Action::Key { keys } => {
                    ensure!(!keys.is_empty() && keys.len() <= 8, "invalid key chord");
                    for key in keys {
                        qcode(key)?;
                    }
                }
                Action::TypeText { text } => {
                    ensure!(text.len() <= 16384 && !text.contains('\0'), "invalid text");
                    unicode_bytes += text.len();
                }
                Action::TypeAscii { text } => {
                    ensure!(
                        text.len() <= 1024
                            && text
                                .chars()
                                .all(|c| c.is_ascii_graphic()
                                    || [' ', '\n', '\r', '\t'].contains(&c)),
                        "console typing needs at most 1024 US-layout ASCII characters"
                    );
                    ascii_bytes += text.len();
                }
                Action::Wait { milliseconds } => {
                    ensure!(*milliseconds <= 10_000, "action wait exceeds 10 seconds");
                    wait += milliseconds;
                }
            }
        }
        ensure!(wait <= 10_000, "batch waits exceed 10 seconds");
        ensure!(
            unicode_bytes <= 16384 && ascii_bytes <= 1024,
            "batch typing exceeds limit"
        );
        Ok(())
    }
}

pub fn qcode(key: &str) -> Result<String> {
    let lower = key.to_lowercase();
    let code = match lower.as_str() {
        "ctrl" | "control" => "ctrl",
        "alt" => "alt",
        "shift" => "shift",
        "win" | "super" | "meta" => "meta_l",
        "enter" | "return" => "ret",
        "esc" | "escape" => "esc",
        "backspace" => "backspace",
        "delete" => "delete",
        "space" => "spc",
        "pageup" => "pgup",
        "pagedown" => "pgdn",
        "arrowup" => "up",
        "arrowdown" => "down",
        "arrowleft" => "left",
        "arrowright" => "right",
        "tab" | "home" | "end" | "up" | "down" | "left" | "right" | "insert" => &lower,
        _ if lower.len() == 1 && lower.as_bytes()[0].is_ascii_alphanumeric() => &lower,
        _ if (1..=12).any(|n| lower == format!("f{n}")) => &lower,
        _ => bail!("unsupported physical key: {key}"),
    };
    Ok(code.into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestRequest {
    pub op: GuestOp,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestOp {
    Health,
    Windows,
    Focus,
    ClipboardGet,
    ClipboardSet,
    TypeText,
    Launch,
    ProcessStart,
    ProcessStatus,
    ProcessKill,
    ProcessForget,
    FileBegin,
    FileWrite,
    FileCommit,
    FileRead,
    FileStat,
    FileAbort,
    TransferBegin,
    TransferStatus,
    TransferCommit,
    TransferAbort,
    TransferPause,
    GraphicsStatus,
    GraphicsTarget,
    GraphicsPrepare,
    GraphicsRun,
    A11y,
}

impl GuestOp {
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Self::Health
                | Self::Windows
                | Self::ClipboardGet
                | Self::ProcessStatus
                | Self::FileRead
                | Self::FileStat
                | Self::A11y
                | Self::TransferStatus
                | Self::GraphicsStatus
                | Self::GraphicsTarget
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mutations_are_not_safe_read_retries() {
        for op in [
            GuestOp::Focus,
            GuestOp::ClipboardSet,
            GuestOp::TypeText,
            GuestOp::Launch,
            GuestOp::ProcessStart,
            GuestOp::ProcessKill,
            GuestOp::ProcessForget,
            GuestOp::FileBegin,
            GuestOp::FileWrite,
            GuestOp::FileCommit,
            GuestOp::FileAbort,
            GuestOp::TransferBegin,
            GuestOp::TransferCommit,
            GuestOp::TransferAbort,
            GuestOp::TransferPause,
            GuestOp::GraphicsPrepare,
            GuestOp::GraphicsRun,
        ] {
            assert!(!op.is_read_only(), "{op:?}");
        }
        for op in [
            GuestOp::Health,
            GuestOp::Windows,
            GuestOp::ClipboardGet,
            GuestOp::ProcessStatus,
            GuestOp::FileRead,
            GuestOp::FileStat,
            GuestOp::A11y,
            GuestOp::TransferStatus,
            GuestOp::GraphicsStatus,
            GuestOp::GraphicsTarget,
        ] {
            assert!(op.is_read_only(), "{op:?}");
        }
    }

    #[test]
    fn strict_actions_and_bounds() {
        assert!(
            serde_json::from_value::<Action>(
                serde_json::json!({"type":"click","x":1,"y":2,"host_path":"/"})
            )
            .is_err()
        );
        let mut b = Batch {
            protocol: 1,
            request_id: "r".into(),
            epoch: "e".into(),
            expected_input_generation: None,
            actions: vec![Action::Click {
                x: 640,
                y: 0,
                button: "left".into(),
            }],
        };
        assert!(b.validate(640, 480).is_err());
        b.actions = vec![
            Action::TypeAscii {
                text: "x".repeat(600),
            },
            Action::TypeAscii {
                text: "x".repeat(600),
            },
        ];
        assert!(b.validate(640, 480).is_err());
        b.actions = vec![
            Action::TypeText {
                text: "x".repeat(9000),
            },
            Action::TypeText {
                text: "x".repeat(9000),
            },
        ];
        assert!(b.validate(640, 480).is_err());
        b.actions = vec![Action::Key {
            keys: vec!["CTRL".into(), "S".into()],
        }];
        b.validate(640, 480).unwrap();
        b.actions.push(Action::Wait {
            milliseconds: 10001,
        });
        assert!(b.validate(640, 480).is_err());
    }
}
