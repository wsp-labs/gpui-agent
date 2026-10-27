//! The messages between the `gpui-agent` command line and the crate inside a GPUI app.
//!
//! One JSON object per line over a Unix socket. Coordinates are logical pixels in window space.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// The variable the command line sets to the socket path; the app serves only when it is set.
pub const SOCKET_ENV: &str = "GPUI_AGENT_SOCKET";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Hello,
    Windows,
    Tree {
        window: u64,
    },
    Input {
        window: u64,
        events: Vec<InputEvent>,
    },
    Screenshot {
        window: u64,
        path: String,
    },
    Actions {
        window: u64,
    },
    Action {
        window: u64,
        name: String,
        data: Option<serde_json::Value>,
    },
    Probe {
        name: String,
    },
    Probes,
    /// Queue the answer to the next file panel: paths, or a cancel when `paths` is empty.
    Pick {
        paths: Vec<String>,
    },
    /// The file panels the app asked for, oldest first.
    Picks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PickRecord {
    /// `open` or `save`.
    pub kind: String,
    /// What the app asked for: the panel's options or its suggested folder and name.
    pub request: String,
    /// The paths given back, or none when the panel was cancelled.
    pub answer: Option<Vec<String>>,
    /// The panel came with nothing queued, so it was cancelled.
    pub unqueued: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Ok { ok: serde_json::Value },
    Err { error: ErrorReply },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorReply {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Recording just turned on; the next frame has not been drawn yet.
    Warming,
    NoWindow,
    NotFound,
    BadRequest,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub pid: u32,
    pub os: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub window: u64,
    pub title: String,
    pub active: bool,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn area(&self) -> f32 {
        self.w.max(0.) * self.h.max(0.)
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0. || self.h <= 0.
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2., self.y + self.h / 2.)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    pub fn contains_rect(&self, other: &Rect) -> bool {
        other.x >= self.x - 0.5
            && other.y >= self.y - 0.5
            && other.x + other.w <= self.x + self.w + 0.5
            && other.y + other.h <= self.y + self.h + 0.5
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Flags {
    #[serde(default, skip_serializing_if = "is_false")]
    pub click: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mouse: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub keys: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hover: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub focusable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub focused: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub scroll: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub drag: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub drop: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tooltip: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub blocks: bool,
}

impl Flags {
    pub fn pointer(&self) -> bool {
        self.click
            || self.mouse
            || self.hover
            || self.scroll
            || self.drag
            || self.drop
            || self.tooltip
    }

    pub fn interactive(&self) -> bool {
        self.click || self.mouse || self.focusable || self.scroll || self.drag || self.drop
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawNode {
    pub parent: Option<usize>,
    pub bounds: Rect,
    pub visible: Rect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    pub flags: Flags,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawText {
    pub bounds: Rect,
    pub visible: Rect,
    pub text: String,
    /// The node that painted this line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawTree {
    pub window: u64,
    pub title: String,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub nodes: Vec<RawNode>,
    pub texts: Vec<RawText>,
    /// Focus handles of nodes that registered a text input handler.
    #[serde(default)]
    pub text_inputs: Vec<String>,
    /// A prompt drawn by GPUI is waiting for an answer.
    #[serde(default)]
    pub prompt: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Modifiers {
    #[serde(default, skip_serializing_if = "is_false")]
    pub shift: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub control: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub alt: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub platform: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputEvent {
    MouseMove {
        x: f32,
        y: f32,
        #[serde(default)]
        pressed: Option<Button>,
        #[serde(default)]
        modifiers: Modifiers,
    },
    MouseDown {
        x: f32,
        y: f32,
        #[serde(default)]
        button: Button,
        #[serde(default = "one")]
        click_count: usize,
        #[serde(default)]
        modifiers: Modifiers,
    },
    MouseUp {
        x: f32,
        y: f32,
        #[serde(default)]
        button: Button,
        #[serde(default = "one")]
        click_count: usize,
        #[serde(default)]
        modifiers: Modifiers,
    },
    Scroll {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        #[serde(default)]
        modifiers: Modifiers,
    },
    /// One keystroke in GPUI's syntax, such as `enter`, `ctrl-a` or `cmd-shift-p`.
    Key { keystroke: String },
    /// Text typed character by character.
    Text { text: String },
}

fn one() -> usize {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionInfo {
    pub name: String,
    pub bindings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
}
