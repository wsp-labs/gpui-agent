//! `gpui-agent mcp`: the same commands as MCP tools over stdio. Each call runs the command line
//! itself, so the tools and the commands cannot drift apart.

use anyhow::Context as _;
use base64::Engine as _;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: Value,
    args: fn(&Value) -> anyhow::Result<Vec<String>>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn need(value: &Value, key: &str) -> anyhow::Result<String> {
    text(value, key).with_context(|| format!("missing {key:?}"))
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn target_schema(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

const TARGET: &str = "A ref like @e3 from snapshot, id:NAME, or text:WORDS";

fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "launch",
            description: "Start a GPUI app that calls gpui_agent::init and wait until it answers. On Linux with no display it runs under a private Xvfb.",
            schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "array", "items": { "type": "string" }, "description": "The app's command and arguments" },
                    "headless": { "type": "boolean" },
                    "env": { "type": "array", "items": { "type": "string" }, "description": "KEY=VALUE pairs" }
                },
                "required": ["command"]
            }),
            args: |a| {
                let mut out = vec!["launch".to_string()];
                if flag(a, "headless") {
                    out.push("--headless".into());
                }
                for pair in strings(a, "env") {
                    out.push("--env".into());
                    out.push(pair);
                }
                out.push("--".into());
                out.extend(strings(a, "command"));
                Ok(out)
            },
        },
        Tool {
            name: "close",
            description: "Stop the session's app and its Xvfb.",
            schema: json!({ "type": "object", "properties": {} }),
            args: |_| Ok(vec!["close".into()]),
        },
        Tool {
            name: "snapshot",
            description: "What is on screen, one line per element with a ref (@e3), its role, its text and the source line that built it. Refs stay attached to their element across snapshots.",
            schema: json!({
                "type": "object",
                "properties": {
                    "interactive": { "type": "boolean", "description": "Only elements that take input" },
                    "all": { "type": "boolean", "description": "Include clipped and scrolled-out elements" },
                    "depth": { "type": "integer" },
                    "scope": target_schema("Only the subtree under this target")
                }
            }),
            args: |a| {
                let mut out = vec!["snapshot".to_string()];
                if flag(a, "interactive") {
                    out.push("-i".into());
                }
                if flag(a, "all") {
                    out.push("-a".into());
                }
                if let Some(depth) = text(a, "depth") {
                    out.extend(["-d".into(), depth]);
                }
                if let Some(scope) = text(a, "scope") {
                    out.extend(["-s".into(), scope]);
                }
                Ok(out)
            },
        },
        Tool {
            name: "click",
            description: "Click an element. Refuses when it is clipped away or covered by something else, and says what covers it.",
            schema: json!({
                "type": "object",
                "properties": { "target": target_schema(TARGET), "right": { "type": "boolean" }, "double": { "type": "boolean" } },
                "required": ["target"]
            }),
            args: |a| {
                let target = need(a, "target")?;
                if flag(a, "double") {
                    return Ok(vec!["dblclick".into(), target]);
                }
                let mut out = vec!["click".to_string(), target];
                if flag(a, "right") {
                    out.push("--right".into());
                }
                Ok(out)
            },
        },
        Tool {
            name: "hover",
            description: "Move the mouse over an element.",
            schema: json!({ "type": "object", "properties": { "target": target_schema(TARGET) }, "required": ["target"] }),
            args: |a| Ok(vec!["hover".into(), need(a, "target")?]),
        },
        Tool {
            name: "fill",
            description: "Click a text field, select all, delete and type the text.",
            schema: json!({
                "type": "object",
                "properties": { "target": target_schema(TARGET), "text": { "type": "string" } },
                "required": ["target", "text"]
            }),
            args: |a| Ok(vec!["fill".into(), need(a, "target")?, need(a, "text")?]),
        },
        Tool {
            name: "type",
            description: "Type text into the focused element, or into target after clicking it, without clearing. With keys, send one keystroke per character, for elements that read keys rather than text input.",
            schema: json!({
                "type": "object",
                "properties": { "text": { "type": "string" }, "target": target_schema(TARGET), "keys": { "type": "boolean" } },
                "required": ["text"]
            }),
            args: |a| {
                let mut out = vec!["type".to_string()];
                if flag(a, "keys") {
                    out.push("--keys".into());
                }
                if let Some(target) = text(a, "target") {
                    out.push(target);
                }
                out.push(need(a, "text")?);
                Ok(out)
            },
        },
        Tool {
            name: "press",
            description: "Press keystrokes in GPUI's syntax, such as enter, ctrl-a, cmd-shift-p or secondary-s (cmd on a Mac, ctrl elsewhere).",
            schema: json!({
                "type": "object",
                "properties": { "keys": { "type": "array", "items": { "type": "string" } } },
                "required": ["keys"]
            }),
            args: |a| {
                let mut out = vec!["press".to_string()];
                out.extend(strings(a, "keys"));
                Ok(out)
            },
        },
        Tool {
            name: "scroll",
            description: "Scroll up, down, left or right over an element or the window's center. With into_view, scroll the target's container until the target is visible.",
            schema: json!({
                "type": "object",
                "properties": {
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "amount": { "type": "number" },
                    "on": target_schema(TARGET),
                    "into_view": target_schema("Scroll until this target is visible")
                }
            }),
            args: |a| {
                if let Some(target) = text(a, "into_view") {
                    return Ok(vec!["scrollintoview".into(), target]);
                }
                let mut out = vec!["scroll".to_string(), need(a, "direction")?];
                if let Some(amount) = text(a, "amount") {
                    out.push(amount);
                }
                if let Some(on) = text(a, "on") {
                    out.extend(["--on".into(), on]);
                }
                Ok(out)
            },
        },
        Tool {
            name: "drag",
            description: "Drag from one element to another.",
            schema: json!({
                "type": "object",
                "properties": { "from": target_schema(TARGET), "to": target_schema(TARGET) },
                "required": ["from", "to"]
            }),
            args: |a| Ok(vec!["drag".into(), need(a, "from")?, need(a, "to")?]),
        },
        Tool {
            name: "screenshot",
            description: "A PNG of the window, rendered by the app itself. With annotate, numbered boxes match snapshot refs ([3] is @e3). With element, only that element.",
            schema: json!({ "type": "object", "properties": { "annotate": { "type": "boolean" }, "element": target_schema(TARGET) } }),
            args: |a| {
                let mut out = vec!["screenshot".to_string()];
                if flag(a, "annotate") {
                    out.push("--annotate".into());
                }
                if let Some(element) = text(a, "element") {
                    out.extend(["--element".into(), element]);
                }
                Ok(out)
            },
        },
        Tool {
            name: "record",
            description: "Record the window from the app's own renderer. action start begins at fps frames a second; action stop writes the video (path ending .mp4 or .gif, needs ffmpeg) and says how many frames it holds.",
            schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["start", "stop"] },
                    "fps": { "type": "integer" },
                    "path": { "type": "string" }
                },
                "required": ["action"]
            }),
            args: |a| {
                let action = need(a, "action")?;
                let mut out = vec!["record".to_string(), action.clone()];
                if action == "start" {
                    if let Some(fps) = text(a, "fps") {
                        out.extend(["--fps".into(), fps]);
                    }
                } else if let Some(path) = text(a, "path") {
                    out.push(path);
                }
                Ok(out)
            },
        },
        Tool {
            name: "apps",
            description: "List apps started with GPUI_AGENT=1. Pass attach (a pid or name) to drive one from this session.",
            schema: json!({ "type": "object", "properties": { "attach": { "type": "string" } } }),
            args: |a| match text(a, "attach") {
                Some(app) => Ok(vec!["attach".into(), app]),
                None => Ok(vec!["list".into()]),
            },
        },
        Tool {
            name: "wait",
            description: "Wait for a target to be visible, a text to appear, a text to go, the frame to stop changing (idle), or a number of milliseconds.",
            schema: json!({
                "type": "object",
                "properties": {
                    "target": target_schema(TARGET),
                    "text": { "type": "string" },
                    "gone": { "type": "string" },
                    "idle": { "type": "boolean" },
                    "ms": { "type": "integer" },
                    "timeout": { "type": "integer", "description": "Milliseconds, default 25000" }
                }
            }),
            args: |a| {
                let mut out = vec!["wait".to_string()];
                if let Some(ms) = text(a, "ms") {
                    out.push(ms);
                } else if let Some(target) = text(a, "target") {
                    out.push(target);
                }
                if let Some(t) = text(a, "text") {
                    out.extend(["--text".into(), t]);
                }
                if let Some(g) = text(a, "gone") {
                    out.extend(["--gone".into(), g]);
                }
                if flag(a, "idle") {
                    out.push("--idle".into());
                }
                if let Some(t) = text(a, "timeout") {
                    out.extend(["--timeout".into(), t]);
                }
                Ok(out)
            },
        },
        Tool {
            name: "get",
            description: "Read text (all text inside a target), box (its bounds), probe (app state the app exposes, by name), title, focus or count (visible lines containing a text).",
            schema: json!({
                "type": "object",
                "properties": {
                    "what": { "type": "string", "enum": ["text", "box", "probe", "title", "focus", "count", "picks"] },
                    "arg": { "type": "string", "description": "The target, probe name or text" }
                },
                "required": ["what"]
            }),
            args: |a| {
                let mut out = vec!["get".to_string(), need(a, "what")?];
                if let Some(arg) = text(a, "arg") {
                    out.push(arg);
                }
                Ok(out)
            },
        },
        Tool {
            name: "find",
            description: "Find elements by text, role, id or source location, and optionally click, dblclick, hover, fill or type into the first match.",
            schema: json!({
                "type": "object",
                "properties": {
                    "by": { "type": "string", "enum": ["text", "role", "id", "source"] },
                    "value": { "type": "string" },
                    "name": { "type": "string", "description": "With role, the name must contain this" },
                    "action": { "type": "string", "enum": ["click", "dblclick", "hover", "fill", "type"] },
                    "action_arg": { "type": "string" }
                },
                "required": ["by", "value"]
            }),
            args: |a| {
                let mut out = vec!["find".to_string(), need(a, "by")?, need(a, "value")?];
                if let Some(name) = text(a, "name") {
                    out.extend(["--name".into(), name]);
                }
                if let Some(action) = text(a, "action") {
                    out.push(action);
                }
                if let Some(arg) = text(a, "action_arg") {
                    out.push(arg);
                }
                Ok(out)
            },
        },
        Tool {
            name: "actions",
            description: "The GPUI actions available at the focused element, with their key bindings.",
            schema: json!({ "type": "object", "properties": {} }),
            args: |_| Ok(vec!["actions".into()]),
        },
        Tool {
            name: "action",
            description: "Dispatch a GPUI action by name, with optional JSON data. Often more reliable than clicking.",
            schema: json!({
                "type": "object",
                "properties": { "name": { "type": "string" }, "data": { "type": "string", "description": "JSON" } },
                "required": ["name"]
            }),
            args: |a| {
                let mut out = vec!["action".to_string(), need(a, "name")?];
                if let Some(data) = text(a, "data") {
                    out.push(data);
                }
                Ok(out)
            },
        },
        Tool {
            name: "pick",
            description: "Queue the answer to the app's next open or save panel. With no paths, the next panel is cancelled. Call before the action that opens the panel; get with what=picks lists every panel the app asked for.",
            schema: json!({
                "type": "object",
                "properties": { "paths": { "type": "array", "items": { "type": "string" } } }
            }),
            args: |a| {
                let mut out = vec!["pick".to_string()];
                out.extend(strings(a, "paths"));
                Ok(out)
            },
        },
        Tool {
            name: "windows",
            description: "List the app's windows. Pass window to select one for later calls.",
            schema: json!({ "type": "object", "properties": { "window": { "type": "integer" } } }),
            args: |a| match text(a, "window") {
                Some(window) => Ok(vec!["select".into(), window]),
                None => Ok(vec!["windows".into()]),
            },
        },
        Tool {
            name: "doctor",
            description: "Check the session, the app, the frame record and screenshots, with the app's last log lines when it is not answering.",
            schema: json!({ "type": "object", "properties": {} }),
            args: |_| Ok(vec!["doctor".into()]),
        },
    ]
}

const INSTRUCTIONS: &str = "Drive a GPUI app. Call launch, then snapshot (interactive: true) to get refs like @e3, act with click, fill, type or press, then wait (idle: true) and snapshot again. Refs stay attached to their element; a call fails with a clear message when the element is gone, hidden or covered.";

fn run_tool(session: &str, tool: &Tool, arguments: &Value) -> Value {
    let args = match (tool.args)(arguments) {
        Ok(args) => args,
        Err(error) => return error_result(&format!("{error:#}")),
    };
    let screenshot = tool.name == "screenshot";
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return error_result(&error.to_string()),
    };
    let mut command = std::process::Command::new(exe);
    command.arg("--session").arg(session);
    if screenshot {
        command.arg("--json");
    }
    command.args(&args);
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => return error_result(&error.to_string()),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return error_result(stderr.trim().trim_start_matches("error: "));
    }
    if screenshot {
        let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
        let path = parsed["path"].as_str().unwrap_or_default().to_string();
        let mut content = Vec::new();
        if let Ok(bytes) = std::fs::read(&path) {
            content.push(json!({
                "type": "image",
                "mimeType": "image/png",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }));
        }
        let mut caption = path.clone();
        if let Some(legend) = parsed["legend"].as_array() {
            for line in legend.iter().filter_map(Value::as_str) {
                caption.push('\n');
                caption.push_str(line);
            }
        }
        content.push(json!({ "type": "text", "text": caption }));
        return json!({ "content": content });
    }
    json!({ "content": [{ "type": "text", "text": stdout.trim_end() }] })
}

fn error_result(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

pub fn serve(session: &str) -> anyhow::Result<()> {
    let tools = tools();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message["method"].as_str().unwrap_or_default();
        let params = &message["params"];
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "gpui-agent", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({
                "tools": tools.iter().map(|tool| json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.schema,
                })).collect::<Vec<_>>()
            })),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or_default();
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                let session = arguments["session"].as_str().unwrap_or(session).to_string();
                match tools.iter().find(|tool| tool.name == name) {
                    Some(tool) => Ok(run_tool(&session, tool, &arguments)),
                    None => Err((-32602, format!("no tool {name:?}"))),
                }
            }
            other => Err((-32601, format!("no method {other:?}"))),
        };
        let reply = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        };
        writeln!(stdout, "{reply}")?;
        stdout.flush()?;
    }
    Ok(())
}
