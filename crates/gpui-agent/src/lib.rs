//! Lets the `gpui-agent` command line see and drive this app.
//!
//! Call [`init`] once at startup. It does nothing unless the app was started with
//! `GPUI_AGENT_SOCKET` set, which `gpui-agent launch` does.

use anyhow::{Context as _, anyhow};
use gpui::{
    AnyWindowHandle, App, AppContext as _, Global, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollEvent, Window,
    point, px,
};
use gpui_agent_protocol::{
    ActionInfo, Button, ErrorKind, ErrorReply, Flags, Hello, InputEvent, PROTOCOL_VERSION,
    PickRecord, RawNode, RawText, RawTree, Rect, Request, Response, SOCKET_ENV, ScreenshotInfo,
    WindowInfo,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;

type Job = (Request, mpsc::Sender<Response>);

/// Starts serving the command line if `GPUI_AGENT_SOCKET` is set.
pub fn init(cx: &mut App) {
    let Ok(path) = std::env::var(SOCKET_ENV) else {
        return;
    };
    let (jobs, receiver) = async_channel::unbounded::<Job>();
    let listener = match bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("gpui-agent: cannot listen on {path}: {error:#}");
            return;
        }
    };
    std::thread::Builder::new()
        .name("gpui-agent".into())
        .spawn(move || accept(listener, jobs))
        .expect("spawn gpui-agent thread");
    // Native dialogs are invisible to the frame record, so prompts draw inside the window.
    cx.set_prompt_builder(gpui::fallback_prompt_renderer);
    let picks = Picks::default();
    cx.set_global(picks.clone());
    cx.set_path_prompt_hook(move |prompt| picks.answer(prompt));
    cx.spawn(async move |cx| {
        while let Ok((request, reply)) = receiver.recv().await {
            let response = cx.update(|cx| handle(request, cx));
            reply.send(response).ok();
        }
    })
    .detach();
}

/// Registers a named reader of app state that `gpui-agent get probe <name>` returns.
pub fn probe(
    cx: &mut App,
    name: impl Into<String>,
    read: impl Fn(&mut App) -> serde_json::Value + 'static,
) {
    cx.default_global::<Probes>()
        .0
        .insert(name.into(), Box::new(read));
}

type ProbeReader = Box<dyn Fn(&mut App) -> serde_json::Value>;

#[derive(Default)]
struct Probes(BTreeMap<String, ProbeReader>);

impl Global for Probes {}

/// Answers file panels from a queue the command line fills, and logs every request.
#[derive(Default, Clone)]
struct Picks(Rc<RefCell<PickState>>);

#[derive(Default)]
struct PickState {
    queue: VecDeque<Option<Vec<PathBuf>>>,
    log: Vec<PickRecord>,
}

impl Global for Picks {}

impl Picks {
    fn answer(&self, prompt: gpui::PathPrompt) -> Option<Vec<PathBuf>> {
        let mut state = self.0.borrow_mut();
        let queued = state.queue.pop_front();
        let unqueued = queued.is_none();
        let answer = queued.flatten();
        let (kind, request) = match &prompt {
            gpui::PathPrompt::Open(options) => (
                "open",
                format!(
                    "files {} directories {} multiple {}",
                    options.files, options.directories, options.multiple
                ),
            ),
            gpui::PathPrompt::Save {
                directory,
                suggested_name,
            } => (
                "save",
                format!(
                    "in {} named {}",
                    directory.display(),
                    suggested_name.as_deref().unwrap_or("(none)")
                ),
            ),
        };
        state.log.push(PickRecord {
            kind: kind.into(),
            request,
            answer: answer.as_ref().map(|paths| {
                paths
                    .iter()
                    .map(|path| path.to_string_lossy().to_string())
                    .collect()
            }),
            unqueued,
        });
        answer
    }
}

fn bind(path: &str) -> anyhow::Result<UnixListener> {
    if std::path::Path::new(path).exists() {
        std::fs::remove_file(path).context("remove stale socket")?;
    }
    Ok(UnixListener::bind(path)?)
}

fn accept(listener: UnixListener, jobs: async_channel::Sender<Job>) {
    for stream in listener.incoming().flatten() {
        let jobs = jobs.clone();
        std::thread::spawn(move || serve(stream, jobs));
    }
}

fn serve(stream: UnixStream, jobs: async_channel::Sender<Job>) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let (reply, answer) = mpsc::channel();
                if jobs.send_blocking((request, reply)).is_err() {
                    return;
                }
                match answer.recv() {
                    Ok(response) => response,
                    Err(_) => return,
                }
            }
            Err(error) => failure(ErrorKind::BadRequest, format!("bad request: {error}")),
        };
        let Ok(mut text) = serde_json::to_string(&response) else {
            return;
        };
        text.push('\n');
        if writer.write_all(text.as_bytes()).is_err() {
            return;
        }
    }
}

fn failure(kind: ErrorKind, message: impl Into<String>) -> Response {
    Response::Err {
        error: ErrorReply {
            kind,
            message: message.into(),
        },
    }
}

struct Refusal(ErrorKind, String);

impl From<anyhow::Error> for Refusal {
    fn from(error: anyhow::Error) -> Self {
        Refusal(ErrorKind::Failed, format!("{error:#}"))
    }
}

fn handle(request: Request, cx: &mut App) -> Response {
    let result = match request {
        Request::Hello => to_value(Hello {
            protocol: PROTOCOL_VERSION,
            pid: std::process::id(),
            os: std::env::consts::OS.into(),
        }),
        Request::Windows => windows(cx),
        Request::Tree { window } => on_window(cx, window, tree),
        Request::Input { window, events } => {
            on_window(cx, window, move |window, cx| input(window, cx, &events))
        }
        Request::Screenshot { window, path } => {
            on_window(cx, window, move |window, _| screenshot(window, &path))
        }
        Request::Actions { window } => on_window(cx, window, actions),
        Request::Action { window, name, data } => on_window(cx, window, move |window, cx| {
            let action = cx
                .build_action(&name, data)
                .map_err(|error| Refusal(ErrorKind::NotFound, error.to_string()))?;
            window.dispatch_action(action, cx);
            Ok(serde_json::Value::Null)
        }),
        Request::Probe { name } => run_probe(cx, &name),
        Request::Pick { paths } => {
            let picks = cx.global::<Picks>().clone();
            let answer =
                (!paths.is_empty()).then(|| paths.into_iter().map(PathBuf::from).collect());
            picks.0.borrow_mut().queue.push_back(answer);
            Ok(serde_json::json!({ "queued": picks.0.borrow().queue.len() }))
        }
        Request::Picks => to_value(cx.global::<Picks>().0.borrow().log.clone()),
        Request::Probes => Ok(serde_json::json!(
            cx.try_global::<Probes>()
                .map(|probes| probes.0.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        )),
    };
    match result {
        Ok(ok) => Response::Ok { ok },
        Err(Refusal(kind, message)) => failure(kind, message),
    }
}

fn to_value(value: impl serde::Serialize) -> Result<serde_json::Value, Refusal> {
    serde_json::to_value(value).map_err(|error| Refusal(ErrorKind::Failed, error.to_string()))
}

fn find_window(cx: &App, id: u64) -> Result<AnyWindowHandle, Refusal> {
    cx.windows()
        .into_iter()
        .find(|handle| handle.window_id().as_u64() == id)
        .ok_or_else(|| Refusal(ErrorKind::NoWindow, format!("no window {id}")))
}

fn on_window(
    cx: &mut App,
    id: u64,
    f: impl FnOnce(&mut Window, &mut App) -> Result<serde_json::Value, Refusal>,
) -> Result<serde_json::Value, Refusal> {
    let handle = find_window(cx, id)?;
    cx.update_window(handle, |_, window, cx| f(window, cx))
        .map_err(|error| Refusal(ErrorKind::NoWindow, format!("{error:#}")))?
}

fn windows(cx: &mut App) -> Result<serde_json::Value, Refusal> {
    let active = cx.active_window().map(|handle| handle.window_id().as_u64());
    let mut list = Vec::new();
    for handle in cx.windows() {
        let id = handle.window_id().as_u64();
        let info = cx.update_window(handle, |_, window, _| {
            let size = window.viewport_size();
            WindowInfo {
                window: id,
                title: window.window_title(),
                active: active == Some(id) || window.is_window_active(),
                width: f32::from(size.width),
                height: f32::from(size.height),
                scale: window.scale_factor(),
            }
        });
        if let Ok(info) = info {
            list.push(info);
        }
    }
    to_value(list)
}

fn rect(bounds: gpui::Bounds<Pixels>) -> Rect {
    Rect {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        w: f32::from(bounds.size.width),
        h: f32::from(bounds.size.height),
    }
}

fn tree(window: &mut Window, _cx: &mut App) -> Result<serde_json::Value, Refusal> {
    if !window.driver_enabled() || window.driver_nodes().is_empty() {
        window.set_driver_enabled(true);
        return Err(Refusal(
            ErrorKind::Warming,
            "waiting for the first recorded frame".into(),
        ));
    }
    if window.driver_frame_pending() {
        return Err(Refusal(
            ErrorKind::Warming,
            "waiting for the next frame".into(),
        ));
    }
    let nodes = window
        .driver_nodes()
        .iter()
        .map(|node| RawNode {
            parent: node.parent,
            bounds: rect(node.bounds),
            visible: rect(node.visible_bounds),
            id: node.element_id.as_ref().map(|id| id.to_string()),
            type_name: node.type_name.to_string(),
            source: node
                .source
                .map(|location| format!("{}:{}", location.file(), location.line())),
            role: node.role.map(|role| format!("{role:?}")),
            flags: Flags {
                click: node.flags.click,
                mouse: node.flags.mouse,
                keys: node.flags.keys,
                hover: node.flags.hover,
                focusable: node.flags.focusable,
                focused: node.flags.focused,
                scroll: node.flags.scroll,
                drag: node.flags.drag,
                drop: node.flags.drop,
                tooltip: node.flags.tooltip,
                blocks: node.flags.blocks,
            },
            key_context: node.key_context.as_ref().map(|context| context.to_string()),
            focus: node.focus.map(|focus| format!("{focus:?}")),
        })
        .collect();
    let texts = window
        .driver_texts()
        .iter()
        .map(|text| RawText {
            bounds: rect(text.bounds),
            visible: rect(text.visible_bounds),
            text: text.text.to_string(),
        })
        .collect();
    let size = window.viewport_size();
    to_value(RawTree {
        window: window.window_handle().window_id().as_u64(),
        title: window.window_title(),
        width: f32::from(size.width),
        height: f32::from(size.height),
        scale: window.scale_factor(),
        nodes,
        texts,
        text_inputs: window
            .driver_text_inputs()
            .iter()
            .map(|focus| format!("{focus:?}"))
            .collect(),
        prompt: window.has_active_prompt(),
    })
}

fn position(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

fn modifiers(m: &gpui_agent_protocol::Modifiers) -> gpui::Modifiers {
    gpui::Modifiers {
        control: m.control,
        alt: m.alt,
        shift: m.shift,
        platform: m.platform,
        function: false,
    }
}

fn button(button: Button) -> MouseButton {
    match button {
        Button::Left => MouseButton::Left,
        Button::Right => MouseButton::Right,
        Button::Middle => MouseButton::Middle,
    }
}

fn typed_keystroke(c: char) -> Keystroke {
    let key = match c {
        ' ' => "space".to_string(),
        '\n' => "enter".to_string(),
        '\t' => "tab".to_string(),
        c => c.to_lowercase().collect(),
    };
    Keystroke {
        modifiers: gpui::Modifiers {
            shift: c.is_uppercase(),
            ..Default::default()
        },
        key,
        key_char: Some(c.to_string()),
    }
}

fn input(
    window: &mut Window,
    cx: &mut App,
    events: &[InputEvent],
) -> Result<serde_json::Value, Refusal> {
    let os = cx.platform_info().os;
    let mut handled = Vec::new();
    for event in events {
        match event {
            InputEvent::MouseMove {
                x,
                y,
                pressed,
                modifiers: m,
            } => {
                let result = window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: position(*x, *y),
                        pressed_button: pressed.map(button),
                        modifiers: modifiers(m),
                    }),
                    cx,
                );
                handled.push(!result.propagate);
            }
            InputEvent::MouseDown {
                x,
                y,
                button: b,
                click_count,
                modifiers: m,
            } => {
                let result = window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: button(*b),
                        position: position(*x, *y),
                        modifiers: modifiers(m),
                        click_count: *click_count,
                        first_mouse: false,
                    }),
                    cx,
                );
                handled.push(!result.propagate);
            }
            InputEvent::MouseUp {
                x,
                y,
                button: b,
                click_count,
                modifiers: m,
            } => {
                let result = window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: button(*b),
                        position: position(*x, *y),
                        modifiers: modifiers(m),
                        click_count: *click_count,
                    }),
                    cx,
                );
                handled.push(!result.propagate);
            }
            InputEvent::Scroll {
                x,
                y,
                dx,
                dy,
                modifiers: m,
            } => {
                let result = window.dispatch_event(
                    PlatformInput::Scroll(ScrollEvent {
                        position: position(*x, *y),
                        delta: ScrollDelta::Pixels(point(px(*dx), px(*dy))),
                        modifiers: modifiers(m),
                        ..Default::default()
                    }),
                    cx,
                );
                handled.push(!result.propagate);
            }
            InputEvent::Key { keystroke } => {
                let keystroke = Keystroke::parse_with_os(keystroke, os).map_err(|error| {
                    Refusal(
                        ErrorKind::BadRequest,
                        format!("bad keystroke {keystroke:?}: {error}"),
                    )
                })?;
                handled.push(window.dispatch_keystroke(keystroke, cx));
            }
            InputEvent::Text { text } => {
                for c in text.chars() {
                    handled.push(window.dispatch_keystroke(typed_keystroke(c), cx));
                }
            }
        }
    }
    to_value(serde_json::json!({ "handled": handled }))
}

fn screenshot(window: &mut Window, path: &str) -> Result<serde_json::Value, Refusal> {
    let image = window
        .render_to_image()
        .map_err(|error| anyhow!("render_to_image: {error:#}"))?;
    image.save(path).with_context(|| format!("write {path}"))?;
    to_value(ScreenshotInfo {
        path: path.into(),
        width: image.width(),
        height: image.height(),
    })
}

fn actions(window: &mut Window, cx: &mut App) -> Result<serde_json::Value, Refusal> {
    let os = cx.platform_info().os;
    let list: Vec<ActionInfo> = window
        .available_actions(cx)
        .iter()
        .map(|action| ActionInfo {
            name: action.name().to_string(),
            bindings: window
                .bindings_for_action(action.as_ref())
                .iter()
                .map(|binding| {
                    binding
                        .keystrokes()
                        .iter()
                        .map(|keystroke| keystroke.unparse(os))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect(),
        })
        .collect();
    to_value(list)
}

fn run_probe(cx: &mut App, name: &str) -> Result<serde_json::Value, Refusal> {
    if !cx.has_global::<Probes>() {
        return Err(Refusal(
            ErrorKind::NotFound,
            "the app registers no probes".into(),
        ));
    }
    let probes = cx.remove_global::<Probes>();
    let result = match probes.0.get(name) {
        Some(read) => Ok(read(cx)),
        None => Err(Refusal(
            ErrorKind::NotFound,
            format!(
                "no probe {name:?}; registered: {}",
                probes.0.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        )),
    };
    cx.set_global(probes);
    result
}
