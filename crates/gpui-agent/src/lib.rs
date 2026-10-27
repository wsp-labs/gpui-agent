//! Lets the `gpui-agent` command line see and drive this app.
//!
//! Call [`init`] once at startup. It does nothing unless the app was started with
//! `GPUI_AGENT_SOCKET` set, which `gpui-agent launch` does.

#[cfg(all(feature = "zed", feature = "gpui-pre-0-3-9"))]
compile_error!("gpui-agent builds against one GPUI: enable `zed` or `gpui-pre-0-3-9`, not both");
#[cfg(not(any(feature = "zed", feature = "gpui-pre-0-3-9")))]
compile_error!("gpui-agent needs a GPUI: enable the `zed` or `gpui-pre-0-3-9` feature");

mod compat;

use anyhow::{Context as _, anyhow};
use gpui::{
    AnyWindowHandle, App, AppContext as _, Global, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, Window, point, px,
};
use gpui_agent_protocol::{
    ANNOUNCE_ENV, ActionInfo, Announcement, Button, ErrorKind, ErrorReply, Flags, Hello,
    InputEvent, PROTOCOL_VERSION, PickRecord, RawNode, RawText, RawTree, Recording, Rect, Request,
    Response, SOCKET_ENV, ScreenshotInfo, WindowInfo, apps_dir,
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
    let Some(path) = socket_path() else {
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

/// The socket to serve on: the one `gpui-agent launch` named, or with `GPUI_AGENT=1` one in the
/// shared apps folder, announced there so `gpui-agent attach` can find this app.
fn socket_path() -> Option<String> {
    if let Ok(path) = std::env::var(SOCKET_ENV) {
        return Some(path);
    }
    if std::env::var(ANNOUNCE_ENV).ok().as_deref() != Some("1") {
        return None;
    }
    let dir = apps_dir();
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("gpui-agent: cannot create {}: {error}", dir.display());
        return None;
    }
    let pid = std::process::id();
    let socket = dir
        .join(format!("{pid}.sock"))
        .to_string_lossy()
        .to_string();
    let name = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "app".into());
    let announcement = Announcement {
        pid,
        name,
        socket: socket.clone(),
    };
    let written = serde_json::to_string(&announcement)
        .map_err(anyhow::Error::from)
        .and_then(|json| Ok(std::fs::write(dir.join(format!("{pid}.json")), json)?));
    if let Err(error) = written {
        eprintln!("gpui-agent: cannot announce this app: {error:#}");
    }
    Some(socket)
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
        Request::RecordStart { window, dir, fps } => record_start(cx, window, dir, fps),
        Request::RecordStop => {
            let recorder = cx.try_global::<Recorder>().cloned();
            match recorder {
                Some(recorder) if recorder.0.borrow().task.is_some() => {
                    let mut state = recorder.0.borrow_mut();
                    state.task = None;
                    to_value(Recording {
                        dir: state.dir.clone(),
                        frames: state.frames.get(),
                        fps: state.fps,
                        seconds: state
                            .started
                            .map(|started| started.elapsed().as_secs_f32())
                            .unwrap_or_default(),
                    })
                }
                _ => Err(Refusal(ErrorKind::NotFound, "nothing is recording".into())),
            }
        }
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
            label: node.label.as_ref().map(|label| label.to_string()),
            value: node.value.as_ref().map(|value| value.to_string()),
            disabled: node.disabled,
            checked: node.toggled.map(|toggled| {
                match toggled {
                    gpui::Toggled::True => "checked",
                    gpui::Toggled::False => "unchecked",
                    gpui::Toggled::Mixed => "mixed",
                }
                .to_string()
            }),
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
        .driver_nodes()
        .iter()
        .enumerate()
        .flat_map(|(index, node)| {
            node.texts.iter().map(move |text| RawText {
                bounds: rect(text.bounds),
                visible: rect(text.visible_bounds),
                text: text.text.to_string(),
                node: Some(index),
            })
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
                    compat::scroll(
                        position(*x, *y),
                        ScrollDelta::Pixels(point(px(*dx), px(*dy))),
                        modifiers(m),
                    ),
                    cx,
                );
                handled.push(!result.propagate);
            }
            InputEvent::Key { keystroke } => {
                let keystroke = compat::parse_keystroke(keystroke, cx).map_err(|error| {
                    Refusal(
                        ErrorKind::BadRequest,
                        format!("bad keystroke {keystroke:?}: {error}"),
                    )
                })?;
                handled.push(window.dispatch_keystroke(keystroke, cx));
            }
            InputEvent::Text { text, keys } => {
                if !keys && window.driver_insert_text(text, cx) {
                    handled.push(true);
                } else {
                    for c in text.chars() {
                        handled.push(window.dispatch_keystroke(typed_keystroke(c), cx));
                    }
                }
            }
            InputEvent::ReplaceText { text } => {
                if !window.driver_replace_text(text, cx) {
                    return Err(Refusal(
                        ErrorKind::NotFound,
                        "no text field has focus; click one first".into(),
                    ));
                }
                handled.push(true);
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
                        .map(|keystroke| compat::unparse(keystroke, cx))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect(),
        })
        .collect();
    to_value(list)
}

/// Saves frames from the app's own renderer while it runs, so recording needs no screen.
#[derive(Default, Clone)]
struct Recorder(Rc<RefCell<RecorderState>>);

#[derive(Default)]
struct RecorderState {
    task: Option<gpui::Task<()>>,
    dir: String,
    fps: u32,
    frames: Rc<std::cell::Cell<usize>>,
    started: Option<std::time::Instant>,
}

impl Global for Recorder {}

fn record_start(
    cx: &mut App,
    window: u64,
    dir: String,
    fps: u32,
) -> Result<serde_json::Value, Refusal> {
    let handle = find_window(cx, window)?;
    let fps = fps.clamp(1, 30);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {dir}"))?;
    if cx
        .try_global::<Recorder>()
        .is_some_and(|recorder| recorder.0.borrow().task.is_some())
    {
        return Err(Refusal(ErrorKind::BadRequest, "already recording".into()));
    }
    let frames = Rc::new(std::cell::Cell::new(0));
    let task = cx.spawn({
        let dir = dir.clone();
        let frames = frames.clone();
        async move |cx| {
            let interval = std::time::Duration::from_millis(1000 / fps as u64);
            let mut next = std::time::Instant::now();
            loop {
                let image = cx.update_window(handle, |_, window, _| window.render_to_image());
                let Ok(Ok(image)) = image else {
                    break;
                };
                let path = format!("{dir}/frame-{:05}.png", frames.get());
                frames.set(frames.get() + 1);
                cx.background_spawn(async move {
                    image.save(&path).ok();
                })
                .detach();
                next += interval;
                let now = std::time::Instant::now();
                if next > now {
                    cx.background_executor().timer(next - now).await;
                } else {
                    next = now;
                }
            }
        }
    });
    let recorder = cx.default_global::<Recorder>().clone();
    *recorder.0.borrow_mut() = RecorderState {
        task: Some(task),
        dir: dir.clone(),
        fps,
        frames,
        started: Some(std::time::Instant::now()),
    };
    to_value(serde_json::json!({ "dir": dir, "fps": fps }))
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
