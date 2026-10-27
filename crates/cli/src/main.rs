mod annotate;
mod mcp;
mod session;
mod snapshot;

use anyhow::{Context as _, anyhow, bail};
use clap::{Parser, Subcommand};
use gpui_agent_protocol::{
    ActionInfo, Button, InputEvent, Modifiers, RawTree, Request, ScreenshotInfo, WindowInfo,
};
use serde::{Deserialize, Serialize};
use session::{Client, Launch, Session};
use snapshot::{Frame, Key, Options, Snapshot};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

const CORE_SKILL: &str = include_str!("../skills/core.md");

#[derive(Parser)]
#[command(
    name = "gpui-agent",
    version,
    about = "See and drive a GPUI app from the command line. Start with `gpui-agent skills get core`."
)]
struct Cli {
    /// The session to act on; each holds one app. Defaults to $GPUI_AGENT_SESSION or "default".
    #[arg(
        long,
        global = true,
        env = "GPUI_AGENT_SESSION",
        default_value = "default"
    )]
    session: String,
    /// The window to act on, from `gpui-agent windows`. Defaults to the selected or active one.
    #[arg(long, global = true)]
    window: Option<u64>,
    /// Print JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start an app that calls gpui_agent::init and wait until it answers.
    Launch {
        /// Run under a private Xvfb display (Linux). Implied when there is no display.
        #[arg(long)]
        headless: bool,
        /// Xvfb screen size and depth.
        #[arg(long, default_value = "1920x1080x24")]
        screen: String,
        /// Extra environment for the app, KEY=VALUE.
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Seconds to wait for the app to answer.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        /// The app's command, after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Drive an app that is already running with GPUI_AGENT_SOCKET set.
    Connect { socket: String },
    /// Stop the session's app and its Xvfb.
    Close,
    /// List the app's windows.
    Windows,
    /// Select the window later commands act on.
    Select { window: u64 },
    /// Print what is on screen, one line per element, each with a ref.
    Snapshot {
        /// Only elements that take input.
        #[arg(short, long)]
        interactive: bool,
        /// Include clipped and scrolled-out elements.
        #[arg(short, long)]
        all: bool,
        /// Stop at this depth.
        #[arg(short, long)]
        depth: Option<usize>,
        /// Only the subtree under this target.
        #[arg(short, long)]
        scope: Option<String>,
        /// Leave out source locations.
        #[arg(long)]
        no_source: bool,
    },
    /// Click an element.
    Click {
        target: String,
        #[arg(long)]
        right: bool,
    },
    /// Double-click an element.
    Dblclick { target: String },
    /// Move the mouse over an element.
    Hover { target: String },
    /// Click a field and replace its text.
    Fill { target: String, text: String },
    /// Type text into the focused element, or into TARGET after clicking it.
    Type {
        /// `[TARGET] TEXT`
        #[arg(num_args = 1..=2, required = true)]
        args: Vec<String>,
        /// Send one keystroke per character, for elements that read keys, not text input.
        #[arg(long)]
        keys: bool,
    },
    /// Press keystrokes in GPUI's syntax: enter, ctrl-a, cmd-shift-p, secondary-s.
    Press {
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Scroll up, down, left or right, over TARGET or the window's center.
    Scroll {
        direction: String,
        #[arg(default_value_t = 300.)]
        amount: f32,
        #[arg(long)]
        on: Option<String>,
    },
    /// Scroll an element's scroll container until the element is visible.
    Scrollintoview { target: String },
    /// Drag from one element to another.
    Drag { from: String, to: String },
    /// Save a PNG of the window.
    Screenshot {
        /// Where to save the PNG; defaults to the session folder.
        path: Option<String>,
        /// Draw numbered boxes keyed to snapshot refs.
        #[arg(long)]
        annotate: bool,
        /// Crop to one element.
        #[arg(long)]
        element: Option<String>,
    },
    /// Record the window to a video: `record start`, then `record stop [out.mp4|out.gif]`.
    Record {
        #[command(subcommand)]
        command: RecordCommand,
    },
    /// List apps started with GPUI_AGENT=1 that this session can attach to.
    List,
    /// Drive an app from `gpui-agent list`, by pid or name.
    Attach { app: String },
    /// Wait for an element, a text, its absence, a still frame, or a number of milliseconds.
    Wait {
        /// A target, or a number of milliseconds.
        target: Option<String>,
        /// Until a visible line of text contains this.
        #[arg(long)]
        text: Option<String>,
        /// Until no visible line of text contains this.
        #[arg(long)]
        gone: Option<String>,
        /// Until the frame stops changing for 300 ms.
        #[arg(long)]
        idle: bool,
        /// Milliseconds before giving up.
        #[arg(long, default_value_t = 25000)]
        timeout: u64,
    },
    /// Read something: text, box, probe, title, focus, count, picks.
    Get {
        /// text, box, probe, title, focus, count or picks
        what: String,
        /// The target, probe name or text it needs.
        arg: Option<String>,
    },
    /// Find elements by text, role, id or source, and optionally act on the first.
    Find {
        /// text, role, id or source
        by: String,
        /// What to look for.
        value: String,
        /// With `role`, the element's name must contain this.
        #[arg(long)]
        name: Option<String>,
        /// click, dblclick, hover, fill or type
        action: Option<String>,
        /// The text for fill or type.
        action_arg: Option<String>,
    },
    /// List the actions available at the focused element, with their key bindings.
    Actions,
    /// Dispatch an action by name, with optional JSON data.
    Action { name: String, data: Option<String> },
    /// Answer the app's next file panel with these paths; with none, cancel it.
    Pick { paths: Vec<String> },
    /// List the app's probes.
    Probes,
    /// Print the raw frame record as JSON.
    Tree,
    /// Check the session, the app and this computer.
    Doctor,
    /// Print the usage guide that matches this version.
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
    /// Serve these commands as MCP tools over stdio.
    Mcp,
    /// List sessions or make a session name for a folder.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
}

#[derive(Subcommand)]
enum RecordCommand {
    /// Start saving frames from the app's own renderer.
    Start {
        /// Frames a second, 1 to 30.
        #[arg(long, default_value_t = 10)]
        fps: u32,
    },
    /// Stop and write the video.
    Stop {
        /// The video to write; `.mp4` or `.gif`. Needs ffmpeg; without it the frames stay as PNGs.
        path: Option<String>,
    },
}

#[derive(Subcommand)]
enum SkillsCommand {
    /// The guides this version carries.
    List,
    /// Print one guide.
    Get { name: String },
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Sessions with state on this computer.
    List,
    /// A stable name for the current git worktree, so parallel agents get separate apps.
    Id {
        #[arg(long, default_value = "gpui")]
        prefix: String,
    },
}

/// Refs stay attached to their element for the life of a session, so a ref from any earlier
/// snapshot, find or annotated screenshot means the same element.
#[derive(Serialize, Deserialize, Default)]
struct Refs {
    window: u64,
    #[serde(default)]
    last: usize,
    entries: Vec<RefEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
struct RefEntry {
    #[serde(rename = "ref")]
    reference: String,
    key: Key,
    role: String,
    name: String,
}

struct Ctx {
    session: Session,
    window: Option<u64>,
    json: bool,
}

fn main() {
    let cli = Cli::parse();
    let result = run(cli);
    if let Err(error) = result {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let session = Session::open(&cli.session)?;
    let mut ctx = Ctx {
        session,
        window: cli.window,
        json: cli.json,
    };
    match cli.command {
        Command::Launch {
            headless,
            screen,
            env,
            timeout,
            command,
        } => launch(&mut ctx, headless, screen, env, timeout, command),
        Command::Connect { socket } => {
            ctx.session.write("socket", &socket)?;
            let mut client = ctx.session.connect()?;
            let hello: gpui_agent_protocol::Hello = client.call(&Request::Hello)?;
            say(&ctx, &hello, || format!("connected to pid {}", hello.pid))
        }
        Command::Close => {
            let stopped = session::close(&ctx.session)?;
            say(&ctx, &serde_json::json!({ "stopped": stopped }), || {
                if stopped {
                    "closed".into()
                } else {
                    "nothing was running".into()
                }
            })
        }
        Command::Windows => {
            let mut client = ctx.session.connect()?;
            let windows: Vec<WindowInfo> = client.call(&Request::Windows)?;
            say(&ctx, &windows, || {
                windows
                    .iter()
                    .map(|w| {
                        format!(
                            "{} {:?} {}x{} scale {}{}",
                            w.window,
                            w.title,
                            w.width,
                            w.height,
                            w.scale,
                            if w.active { " active" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        Command::Select { window } => {
            let mut client = ctx.session.connect()?;
            let id = ctx.session.window(&mut client, Some(window))?;
            ctx.session.write("window", &id.to_string())?;
            say(&ctx, &serde_json::json!({ "window": id }), || {
                format!("selected window {id}")
            })
        }
        Command::Snapshot {
            interactive,
            all,
            depth,
            scope,
            no_source,
        } => {
            let (mut client, window) = connect(&ctx)?;
            let tree = client.tree(window)?;
            let scope = match scope {
                Some(target) => {
                    let frame = Frame::new(&tree);
                    let node = resolve(&ctx, &frame, &target)?;
                    Some(frame.keys[node].clone())
                }
                None => None,
            };
            let snapshot = snapshot::build(
                &tree,
                &Options {
                    interactive,
                    all,
                    depth,
                    scope,
                },
            );
            let mut snapshot = snapshot;
            assign_refs(&ctx, &mut snapshot)?;
            if ctx.json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                print!("{}", snapshot::render(&snapshot, !no_source));
            }
            Ok(())
        }
        Command::Click { target, right } => {
            let button = if right { Button::Right } else { Button::Left };
            pointer(
                &ctx,
                &target,
                |x, y| click_events(x, y, button, 1),
                "clicked",
            )
        }
        Command::Dblclick { target } => pointer(
            &ctx,
            &target,
            |x, y| {
                let mut events = click_events(x, y, Button::Left, 1);
                events.extend(click_events(x, y, Button::Left, 2).into_iter().skip(1));
                events
            },
            "double-clicked",
        ),
        Command::Hover { target } => pointer(
            &ctx,
            &target,
            |x, y| {
                vec![InputEvent::MouseMove {
                    x,
                    y,
                    pressed: None,
                    modifiers: Modifiers::default(),
                }]
            },
            "hovered",
        ),
        Command::Fill { target, text } => fill(&ctx, &target, &text),
        Command::Type { args, keys } => {
            let (target, text) = match args.as_slice() {
                [text] => (None, text.clone()),
                [target, text] => (Some(target.clone()), text.clone()),
                _ => unreachable!(),
            };
            if let Some(target) = &target {
                pointer(
                    &ctx,
                    target,
                    |x, y| click_events(x, y, Button::Left, 1),
                    "clicked",
                )?;
            }
            let (mut client, window) = connect(&ctx)?;
            client.tree(window)?;
            send(
                &mut client,
                window,
                vec![InputEvent::Text {
                    text: text.clone(),
                    keys,
                }],
            )?;
            say(&ctx, &serde_json::json!({ "typed": text }), || {
                format!("typed {text:?}")
            })
        }
        Command::Press { keys } => {
            let (mut client, window) = connect(&ctx)?;
            let events = keys
                .iter()
                .map(|keystroke| InputEvent::Key {
                    keystroke: keystroke.clone(),
                })
                .collect();
            let handled = send(&mut client, window, events)?;
            say(
                &ctx,
                &serde_json::json!({ "pressed": keys, "handled": handled }),
                || format!("pressed {}", keys.join(" ")),
            )
        }
        Command::Scroll {
            direction,
            amount,
            on,
        } => scroll(&ctx, &direction, amount, on.as_deref()),
        Command::Scrollintoview { target } => scroll_into_view(&ctx, &target),
        Command::Drag { from, to } => drag(&ctx, &from, &to),
        Command::Screenshot {
            path,
            annotate,
            element,
        } => screenshot(&ctx, path, annotate, element),
        Command::Record { command } => record(&ctx, command),
        Command::List => {
            let apps = announced_apps();
            say(&ctx, &apps, || {
                if apps.is_empty() {
                    "no apps announced; start one with GPUI_AGENT=1".into()
                } else {
                    apps.iter()
                        .map(|app| format!("{} pid {}", app.name, app.pid))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            })
        }
        Command::Attach { app } => {
            let apps = announced_apps();
            let found = apps
                .iter()
                .find(|a| a.pid.to_string() == app)
                .or_else(|| apps.iter().find(|a| a.name == app))
                .ok_or_else(|| anyhow!("no announced app {app:?}; `gpui-agent list` shows them"))?;
            ctx.session.write("socket", &found.socket)?;
            std::fs::remove_file(ctx.session.file("window")).ok();
            std::fs::remove_file(ctx.session.file("refs.json")).ok();
            let mut client = ctx.session.connect()?;
            let hello: gpui_agent_protocol::Hello = client.call(&Request::Hello)?;
            say(&ctx, &hello, || {
                format!("attached to {} pid {}", found.name, hello.pid)
            })
        }
        Command::Wait {
            target,
            text,
            gone,
            idle,
            timeout,
        } => wait(
            &ctx,
            target,
            text,
            gone,
            idle,
            Duration::from_millis(timeout),
        ),
        Command::Get { what, arg } => get(&ctx, &what, arg.as_deref()),
        Command::Find {
            by,
            value,
            name,
            action,
            action_arg,
        } => find(
            &ctx,
            &by,
            &value,
            name.as_deref(),
            action.as_deref(),
            action_arg,
        ),
        Command::Actions => {
            let (mut client, window) = connect(&ctx)?;
            let actions: Vec<ActionInfo> = client.call(&Request::Actions { window })?;
            say(&ctx, &actions, || {
                actions
                    .iter()
                    .map(|a| {
                        if a.bindings.is_empty() {
                            a.name.clone()
                        } else {
                            format!("{}  {}", a.name, a.bindings.join(", "))
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        Command::Action { name, data } => {
            let (mut client, window) = connect(&ctx)?;
            let data = data
                .map(|data| serde_json::from_str(&data).context("action data must be JSON"))
                .transpose()?;
            let _: serde_json::Value = client.call(&Request::Action {
                window,
                name: name.clone(),
                data,
            })?;
            say(&ctx, &serde_json::json!({ "dispatched": name }), || {
                format!("dispatched {name}")
            })
        }
        Command::Pick { paths } => {
            let mut client = ctx.session.connect()?;
            let paths: Vec<String> = paths
                .iter()
                .map(|path| {
                    std::path::absolute(path)
                        .map(|path| path.to_string_lossy().to_string())
                        .unwrap_or_else(|_| path.clone())
                })
                .collect();
            let _: serde_json::Value = client.call(&Request::Pick {
                paths: paths.clone(),
            })?;
            say(&ctx, &serde_json::json!({ "queued": paths }), || {
                if paths.is_empty() {
                    "the next file panel will be cancelled".into()
                } else {
                    format!("the next file panel answers {}", paths.join(" "))
                }
            })
        }
        Command::Probes => {
            let mut client = ctx.session.connect()?;
            let names: Vec<String> = client.call(&Request::Probes)?;
            say(&ctx, &names, || names.join("\n"))
        }
        Command::Tree => {
            let (mut client, window) = connect(&ctx)?;
            let tree = client.tree(window)?;
            println!("{}", serde_json::to_string_pretty(&tree)?);
            Ok(())
        }
        Command::Doctor => doctor(&ctx),
        Command::Mcp => mcp::serve(&ctx.session.name),
        Command::Skills { command } => match command {
            SkillsCommand::List => {
                println!("core  the snapshot-and-ref loop, every command, and the traps");
                Ok(())
            }
            SkillsCommand::Get { name } => match name.as_str() {
                "core" => {
                    print!("{CORE_SKILL}");
                    Ok(())
                }
                other => bail!("no skill {other:?}; `gpui-agent skills list` names them"),
            },
        },
        Command::Session { command } => match command {
            SessionCommand::List => {
                let mut names = Vec::new();
                if let Ok(dirs) = std::fs::read_dir(session::state_root()) {
                    for dir in dirs.flatten() {
                        let name = dir.file_name().to_string_lossy().to_string();
                        if !name.starts_with('.') {
                            names.push(name);
                        }
                    }
                }
                names.sort();
                say(&ctx, &names, || names.join("\n"))
            }
            SessionCommand::Id { prefix } => {
                let top = std::process::Command::new("git")
                    .args(["rev-parse", "--show-toplevel"])
                    .output()
                    .ok()
                    .filter(|out| out.status.success())
                    .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                    .unwrap_or_else(|| {
                        std::env::current_dir()
                            .map(|dir| dir.to_string_lossy().to_string())
                            .unwrap_or_default()
                    });
                let mut hasher = DefaultHasher::new();
                top.hash(&mut hasher);
                let base = std::path::Path::new(&top)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect::<String>();
                println!("{prefix}-{base}-{:08x}", hasher.finish() as u32);
                Ok(())
            }
        },
    }
}

fn say<T: Serialize>(ctx: &Ctx, value: &T, text: impl FnOnce() -> String) -> anyhow::Result<()> {
    if ctx.json {
        println!("{}", serde_json::to_string(value)?);
    } else {
        let text = text();
        if !text.is_empty() {
            println!("{text}");
        }
    }
    Ok(())
}

fn connect(ctx: &Ctx) -> anyhow::Result<(Client, u64)> {
    let mut client = ctx.session.connect()?;
    let window = ctx.session.window(&mut client, ctx.window)?;
    Ok((client, window))
}

fn launch(
    ctx: &mut Ctx,
    headless: bool,
    screen: String,
    env: Vec<String>,
    timeout: u64,
    command: Vec<String>,
) -> anyhow::Result<()> {
    let env = env
        .into_iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .ok_or_else(|| anyhow!("--env takes KEY=VALUE, got {pair:?}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let (hello, pid, windows) = session::launch(
        &ctx.session,
        Launch {
            command,
            headless,
            screen,
            env,
            timeout: Duration::from_secs(timeout),
        },
    )?;
    if let Some(first) = windows.iter().find(|w| w.active).or(windows.first()) {
        ctx.session.write("window", &first.window.to_string())?;
    }
    let display = ctx.session.read("display");
    say(
        ctx,
        &serde_json::json!({ "pid": pid, "os": hello.os, "display": display, "windows": windows }),
        || {
            let mut text = format!("launched pid {pid}");
            if let Some(display) = &display {
                text.push_str(&format!(" on Xvfb {display}"));
            }
            for w in &windows {
                text.push_str(&format!(
                    "\nwindow {} {:?} {}x{}",
                    w.window, w.title, w.width, w.height
                ));
            }
            if windows.is_empty() {
                text.push_str("\nno window opened yet");
            }
            text
        },
    )
}

fn assign_refs(ctx: &Ctx, snapshot: &mut Snapshot) -> anyhow::Result<()> {
    let mut refs = load_refs(ctx)
        .ok()
        .filter(|refs| refs.window == snapshot.window)
        .unwrap_or_else(|| Refs {
            window: snapshot.window,
            ..Default::default()
        });
    let mut by_key: std::collections::HashMap<Key, usize> = refs
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| (entry.key.clone(), i))
        .collect();
    for entry in &mut snapshot.entries {
        let index = match by_key.get(&entry.key) {
            Some(&index) => index,
            None => {
                refs.last += 1;
                refs.entries.push(RefEntry {
                    reference: format!("e{}", refs.last),
                    key: entry.key.clone(),
                    role: String::new(),
                    name: String::new(),
                });
                by_key.insert(entry.key.clone(), refs.entries.len() - 1);
                refs.entries.len() - 1
            }
        };
        let stored = &mut refs.entries[index];
        stored.role = entry.role.clone();
        stored.name = entry.name.clone();
        entry.reference = stored.reference.clone();
    }
    ctx.session
        .write("refs.json", &serde_json::to_string(&refs)?)
}

fn load_refs(ctx: &Ctx) -> anyhow::Result<Refs> {
    let text = ctx
        .session
        .read("refs.json")
        .ok_or_else(|| anyhow!("no snapshot yet; run `gpui-agent snapshot` to get refs"))?;
    Ok(serde_json::from_str(&text)?)
}

/// Finds the node a target names in the current frame. Targets are `@e3`, `id:name` or
/// `text:words`.
fn resolve(ctx: &Ctx, frame: &Frame, target: &str) -> anyhow::Result<usize> {
    if let Some(reference) = target.strip_prefix('@') {
        let refs = load_refs(ctx)?;
        let entry = refs
            .entries
            .iter()
            .find(|entry| entry.reference == reference)
            .ok_or_else(|| {
                anyhow!("no ref @{reference} in the last snapshot; run `gpui-agent snapshot`")
            })?;
        if refs.window != frame.tree.window {
            bail!(
                "@{reference} belongs to window {}, not {}; run `gpui-agent snapshot`",
                refs.window,
                frame.tree.window
            );
        }
        return frame.find(&entry.key).ok_or_else(|| {
            anyhow!(
                "@{reference} ({} {:?}) is no longer on screen; run `gpui-agent snapshot` again",
                entry.role,
                entry.name
            )
        });
    }
    let snapshot = snapshot::build(
        frame.tree,
        &Options {
            all: true,
            ..Default::default()
        },
    );
    let matched = if let Some(id) = target.strip_prefix("id:") {
        snapshot
            .entries
            .iter()
            .find(|entry| entry.key.path == id || entry.key.path.ends_with(&format!("/{id}")))
    } else if let Some(text) = target.strip_prefix("text:") {
        snapshot
            .entries
            .iter()
            .find(|entry| entry.name == text)
            .or_else(|| {
                snapshot
                    .entries
                    .iter()
                    .find(|entry| entry.name.contains(text))
            })
    } else {
        bail!("targets are @e3 refs from a snapshot, id:NAME or text:WORDS; got {target:?}")
    };
    matched
        .map(|entry| entry.node)
        .ok_or_else(|| anyhow!("nothing on screen matches {target:?}"))
}

fn describe(frame: &Frame, node: usize) -> String {
    format!("{} {:?}", frame.role(node), frame.name(node))
}

fn click_events(x: f32, y: f32, button: Button, click_count: usize) -> Vec<InputEvent> {
    vec![
        InputEvent::MouseMove {
            x,
            y,
            pressed: None,
            modifiers: Modifiers::default(),
        },
        InputEvent::MouseDown {
            x,
            y,
            button,
            click_count,
            modifiers: Modifiers::default(),
        },
        InputEvent::MouseUp {
            x,
            y,
            button,
            click_count,
            modifiers: Modifiers::default(),
        },
    ]
}

fn send(client: &mut Client, window: u64, events: Vec<InputEvent>) -> anyhow::Result<Vec<bool>> {
    #[derive(Deserialize)]
    struct Handled {
        handled: Vec<bool>,
    }
    let handled: Handled = client.call(&Request::Input { window, events })?;
    Ok(handled.handled)
}

/// Where to put the mouse for a node, refusing when it is hidden or covered.
fn aim(frame: &Frame, node: usize, target: &str) -> anyhow::Result<(f32, f32)> {
    let visible = frame.tree.nodes[node].visible;
    if visible.is_empty() {
        bail!(
            "{target} ({}) is not visible; `gpui-agent scrollintoview {target}` brings it into view",
            describe(frame, node)
        );
    }
    let (x, y) = visible.center();
    if let Some(top) = frame.hit(x, y)
        && top != node
        && !frame.is_ancestor(node, top)
        && !frame.is_ancestor(top, node)
    {
        bail!(
            "{target} ({}) is covered by {} from {}; deal with that first",
            describe(frame, node),
            describe(frame, top),
            frame.tree.nodes[top]
                .source
                .as_deref()
                .unwrap_or("unknown source")
        );
    }
    Ok((x, y))
}

fn pointer(
    ctx: &Ctx,
    target: &str,
    events: impl FnOnce(f32, f32) -> Vec<InputEvent>,
    verb: &str,
) -> anyhow::Result<()> {
    let (mut client, window) = connect(ctx)?;
    let tree = client.tree(window)?;
    let frame = Frame::new(&tree);
    let node = resolve(ctx, &frame, target)?;
    let (x, y) = aim(&frame, node, target)?;
    let what = describe(&frame, node);
    send(&mut client, window, events(x, y))?;
    say(
        ctx,
        &serde_json::json!({ "target": target, "x": x, "y": y, "element": what }),
        || format!("{verb} {target} {what}"),
    )
}

fn fill(ctx: &Ctx, target: &str, text: &str) -> anyhow::Result<()> {
    let (mut client, window) = connect(ctx)?;
    let tree = client.tree(window)?;
    let frame = Frame::new(&tree);
    let node = resolve(ctx, &frame, target)?;
    let (x, y) = aim(&frame, node, target)?;
    send(&mut client, window, click_events(x, y, Button::Left, 1))?;
    client.tree(window)?;
    send(
        &mut client,
        window,
        vec![InputEvent::ReplaceText { text: text.into() }],
    )
    .with_context(|| format!("{target} took no text after the click"))?;
    say(
        ctx,
        &serde_json::json!({ "target": target, "text": text }),
        || format!("filled {target} with {text:?}"),
    )
}

fn scroll_event(x: f32, y: f32, direction: &str, amount: f32) -> anyhow::Result<InputEvent> {
    let (dx, dy) = match direction {
        "down" => (0., -amount),
        "up" => (0., amount),
        "right" => (-amount, 0.),
        "left" => (amount, 0.),
        other => bail!("scroll directions are up, down, left and right; got {other:?}"),
    };
    Ok(InputEvent::Scroll {
        x,
        y,
        dx,
        dy,
        modifiers: Modifiers::default(),
    })
}

fn scroll(ctx: &Ctx, direction: &str, amount: f32, on: Option<&str>) -> anyhow::Result<()> {
    let (mut client, window) = connect(ctx)?;
    let tree = client.tree(window)?;
    let frame = Frame::new(&tree);
    let (x, y) = match on {
        Some(target) => {
            let node = resolve(ctx, &frame, target)?;
            frame.tree.nodes[node].visible.center()
        }
        None => (tree.width / 2., tree.height / 2.),
    };
    send(
        &mut client,
        window,
        vec![scroll_event(x, y, direction, amount)?],
    )?;
    say(
        ctx,
        &serde_json::json!({ "direction": direction, "amount": amount }),
        || format!("scrolled {direction} {amount}"),
    )
}

fn scroll_into_view(ctx: &Ctx, target: &str) -> anyhow::Result<()> {
    let (mut client, window) = connect(ctx)?;
    for _ in 0..20 {
        let tree = client.tree(window)?;
        let frame = Frame::new(&tree);
        let node = resolve(ctx, &frame, target)?;
        let bounds = frame.tree.nodes[node].bounds;
        let visible = frame.tree.nodes[node].visible;
        let Some(container) = frame.nearest_scroll_ancestor(node) else {
            if visible.is_empty() {
                bail!("{target} is hidden but has no scroll container to bring it into view");
            }
            return say(ctx, &serde_json::json!({ "target": target }), || {
                format!("{target} is visible")
            });
        };
        let area = frame.tree.nodes[container].visible;
        if area.contains_rect(&bounds) {
            return say(ctx, &serde_json::json!({ "target": target }), || {
                format!("{target} is visible")
            });
        }
        let (dx, dy) = (
            if bounds.x < area.x {
                area.x - bounds.x
            } else if bounds.x + bounds.w > area.x + area.w {
                (area.x + area.w) - (bounds.x + bounds.w)
            } else {
                0.
            },
            if bounds.y < area.y {
                area.y - bounds.y
            } else if bounds.y + bounds.h > area.y + area.h {
                (area.y + area.h) - (bounds.y + bounds.h)
            } else {
                0.
            },
        );
        let (x, y) = area.center();
        send(
            &mut client,
            window,
            vec![InputEvent::Scroll {
                x,
                y,
                dx,
                dy,
                modifiers: Modifiers::default(),
            }],
        )?;
    }
    bail!("{target} did not come into view after 20 scrolls")
}

fn drag(ctx: &Ctx, from: &str, to: &str) -> anyhow::Result<()> {
    let (mut client, window) = connect(ctx)?;
    let tree = client.tree(window)?;
    let frame = Frame::new(&tree);
    let start = resolve(ctx, &frame, from)?;
    let end = resolve(ctx, &frame, to)?;
    let (x0, y0) = aim(&frame, start, from)?;
    let (x1, y1) = frame.tree.nodes[end].visible.center();
    let mut events = vec![
        InputEvent::MouseMove {
            x: x0,
            y: y0,
            pressed: None,
            modifiers: Modifiers::default(),
        },
        InputEvent::MouseDown {
            x: x0,
            y: y0,
            button: Button::Left,
            click_count: 1,
            modifiers: Modifiers::default(),
        },
    ];
    for step in 1..=10 {
        let t = step as f32 / 10.;
        events.push(InputEvent::MouseMove {
            x: x0 + (x1 - x0) * t,
            y: y0 + (y1 - y0) * t,
            pressed: Some(Button::Left),
            modifiers: Modifiers::default(),
        });
    }
    events.push(InputEvent::MouseUp {
        x: x1,
        y: y1,
        button: Button::Left,
        click_count: 1,
        modifiers: Modifiers::default(),
    });
    send(&mut client, window, events)?;
    say(ctx, &serde_json::json!({ "from": from, "to": to }), || {
        format!("dragged {from} to {to}")
    })
}

fn screenshot(
    ctx: &Ctx,
    path: Option<String>,
    annotate: bool,
    element: Option<String>,
) -> anyhow::Result<()> {
    let (mut client, window) = connect(ctx)?;
    let path = match path {
        Some(path) => std::path::absolute(path)?.to_string_lossy().to_string(),
        None => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis();
            ctx.session
                .file(&format!("screenshot-{stamp}.png"))
                .to_string_lossy()
                .to_string()
        }
    };
    let info: ScreenshotInfo = client.call(&Request::Screenshot {
        window,
        path: path.clone(),
    })?;
    let mut legend = Vec::new();
    let mut size = (info.width, info.height);
    if let Some(target) = &element {
        let tree = client.tree(window)?;
        let frame = Frame::new(&tree);
        let node = resolve(ctx, &frame, target)?;
        let v = frame.tree.nodes[node].visible;
        anyhow::ensure!(!v.is_empty(), "{target} is not visible");
        let mut image = image::open(&path)?.to_rgba8();
        let scale = image.width() as f32 / tree.width.max(1.);
        let x = ((v.x * scale).round().max(0.) as u32).min(image.width() - 1);
        let y = ((v.y * scale).round().max(0.) as u32).min(image.height() - 1);
        let w = ((v.w * scale).round() as u32).clamp(1, image.width() - x);
        let h = ((v.h * scale).round() as u32).clamp(1, image.height() - y);
        image = image::imageops::crop_imm(&image, x, y, w, h).to_image();
        image.save(&path)?;
        size = (w, h);
    }
    if annotate {
        let tree = client.tree(window)?;
        let snapshot = snapshot::build(
            &tree,
            &Options {
                interactive: true,
                ..Default::default()
            },
        );
        let mut snapshot = snapshot;
        assign_refs(ctx, &mut snapshot)?;
        let mut image = image::open(&path)?.to_rgba8();
        legend = annotate::annotate(&mut image, &snapshot);
        image.save(&path)?;
    }
    say(
        ctx,
        &serde_json::json!({ "path": info.path, "width": size.0, "height": size.1, "legend": legend }),
        || {
            let mut text = format!("{} {}x{}", info.path, size.0, size.1);
            for line in &legend {
                text.push('\n');
                text.push_str(line);
            }
            text
        },
    )
}

fn record(ctx: &Ctx, command: RecordCommand) -> anyhow::Result<()> {
    match command {
        RecordCommand::Start { fps } => {
            let (mut client, window) = connect(ctx)?;
            let dir = ctx.session.file("recording");
            std::fs::remove_dir_all(&dir).ok();
            let dir = dir.to_string_lossy().to_string();
            let _: serde_json::Value = client.call(&Request::RecordStart {
                window,
                dir: dir.clone(),
                fps,
            })?;
            say(
                ctx,
                &serde_json::json!({ "recording": dir, "fps": fps }),
                || format!("recording window {window} at {fps} frames a second"),
            )
        }
        RecordCommand::Stop { path } => {
            let mut client = ctx.session.connect()?;
            let recording: gpui_agent_protocol::Recording = client.call(&Request::RecordStop)?;
            std::thread::sleep(Duration::from_millis(300));
            let out = match path {
                Some(path) => std::path::absolute(path)?,
                None => ctx.session.file("recording.mp4"),
            };
            let gif = out.extension().is_some_and(|ext| ext == "gif");
            let rate = if recording.seconds > 0. {
                (recording.frames as f32 / recording.seconds).clamp(1., recording.fps as f32)
            } else {
                recording.fps as f32
            };
            let mut ffmpeg = std::process::Command::new("ffmpeg");
            ffmpeg
                .args(["-y", "-loglevel", "error", "-framerate"])
                .arg(format!("{rate:.3}"))
                .arg("-i")
                .arg(format!("{}/frame-%05d.png", recording.dir));
            if gif {
                ffmpeg.args(["-vf", "split[a][b];[a]palettegen[p];[b][p]paletteuse"]);
            } else {
                ffmpeg.args([
                    "-vf",
                    "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                    "-pix_fmt",
                    "yuv420p",
                ]);
            }
            let encoded = ffmpeg.arg(&out).status();
            let written = matches!(encoded, Ok(status) if status.success());
            say(
                ctx,
                &serde_json::json!({
                    "frames": recording.frames,
                    "video": written.then(|| out.to_string_lossy().to_string()),
                    "frames_dir": recording.dir,
                }),
                || {
                    if written {
                        format!(
                            "{} ({} frames, {:.1}s)",
                            out.display(),
                            recording.frames,
                            recording.seconds
                        )
                    } else {
                        format!(
                            "{} frames in {}; install ffmpeg to get a video",
                            recording.frames, recording.dir
                        )
                    }
                },
            )
        }
    }
}

fn announced_apps() -> Vec<gpui_agent_protocol::Announcement> {
    let dir = gpui_agent_protocol::apps_dir();
    let mut apps = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return apps;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Some(app) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<gpui_agent_protocol::Announcement>(&text).ok())
        else {
            continue;
        };
        if Client::connect(std::path::Path::new(&app.socket)).is_ok() {
            apps.push(app);
        } else {
            std::fs::remove_file(&path).ok();
            std::fs::remove_file(&app.socket).ok();
        }
    }
    apps.sort_by_key(|app| app.pid);
    apps
}

fn frame_hash(tree: &RawTree) -> u64 {
    let mut hasher = DefaultHasher::new();
    serde_json::to_string(&(&tree.nodes, &tree.texts))
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

fn visible_text(tree: &RawTree, needle: &str) -> bool {
    tree.texts
        .iter()
        .any(|text| !text.visible.is_empty() && text.text.contains(needle))
}

fn wait(
    ctx: &Ctx,
    target: Option<String>,
    text: Option<String>,
    gone: Option<String>,
    idle: bool,
    timeout: Duration,
) -> anyhow::Result<()> {
    if let Some(ms) = target.as_deref().and_then(|t| t.parse::<u64>().ok()) {
        std::thread::sleep(Duration::from_millis(ms));
        return say(ctx, &serde_json::json!({ "waited_ms": ms }), || {
            format!("waited {ms}ms")
        });
    }
    let (mut client, window) = connect(ctx)?;
    let start = Instant::now();
    let mut last_hash = None;
    let mut still_since = Instant::now();
    loop {
        let tree = client.tree(window)?;
        let done = if let Some(target) = &target {
            let frame = Frame::new(&tree);
            resolve(ctx, &frame, target)
                .map(|node| !frame.tree.nodes[node].visible.is_empty())
                .unwrap_or(false)
        } else if let Some(text) = &text {
            visible_text(&tree, text)
        } else if let Some(gone) = &gone {
            !visible_text(&tree, gone)
        } else if idle {
            let hash = frame_hash(&tree);
            if last_hash != Some(hash) {
                last_hash = Some(hash);
                still_since = Instant::now();
            }
            still_since.elapsed() >= Duration::from_millis(300)
        } else {
            bail!("wait for a target, --text, --gone, --idle or a number of milliseconds");
        };
        if done {
            let waited = start.elapsed().as_millis();
            return say(ctx, &serde_json::json!({ "waited_ms": waited }), || {
                format!("ready after {waited}ms")
            });
        }
        if start.elapsed() > timeout {
            bail!("timed out after {}ms", timeout.as_millis());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn get(ctx: &Ctx, what: &str, arg: Option<&str>) -> anyhow::Result<()> {
    let need = |name: &str| arg.ok_or_else(|| anyhow!("`get {what}` needs {name}"));
    match what {
        "probe" => {
            let mut client = ctx.session.connect()?;
            let name = need("a probe name")?;
            let value: serde_json::Value = client.call(&Request::Probe { name: name.into() })?;
            println!("{}", serde_json::to_string(&value)?);
            Ok(())
        }
        "title" => {
            let (mut client, window) = connect(ctx)?;
            let tree = client.tree(window)?;
            say(ctx, &serde_json::json!({ "title": tree.title }), || {
                tree.title.clone()
            })
        }
        "text" | "box" => {
            let target = need("a target")?;
            let (mut client, window) = connect(ctx)?;
            let tree = client.tree(window)?;
            let frame = Frame::new(&tree);
            let node = resolve(ctx, &frame, target)?;
            if what == "text" {
                let text = frame.text_below(node, true);
                say(ctx, &serde_json::json!({ "text": text }), || text.clone())
            } else {
                let b = frame.tree.nodes[node].bounds;
                say(ctx, &b, || format!("{} {} {}x{}", b.x, b.y, b.w, b.h))
            }
        }
        "focus" => {
            let (mut client, window) = connect(ctx)?;
            let tree = client.tree(window)?;
            let frame = Frame::new(&tree);
            let focused = tree
                .nodes
                .iter()
                .enumerate()
                .rev()
                .find(|(_, node)| node.flags.focused)
                .map(|(i, node)| (describe(&frame, i), node.source.clone().unwrap_or_default()));
            say(ctx, &focused, || match &focused {
                Some((what, source)) => format!("{what}  {source}"),
                None => "nothing is focused".into(),
            })
        }
        "count" => {
            let text = need("a text")?;
            let (mut client, window) = connect(ctx)?;
            let tree = client.tree(window)?;
            let count = tree
                .texts
                .iter()
                .filter(|t| !t.visible.is_empty() && t.text.contains(text))
                .count();
            say(ctx, &serde_json::json!({ "count": count }), || {
                count.to_string()
            })
        }
        "picks" => {
            let mut client = ctx.session.connect()?;
            let picks: Vec<gpui_agent_protocol::PickRecord> = client.call(&Request::Picks)?;
            say(ctx, &picks, || {
                if picks.is_empty() {
                    return "the app has not asked for a file panel".into();
                }
                picks
                    .iter()
                    .map(|pick| {
                        let answer = match (&pick.answer, pick.unqueued) {
                            (_, true) => {
                                "cancelled, nothing was queued; `gpui-agent pick <path>` first"
                                    .to_string()
                            }
                            (Some(paths), _) => paths.join(" "),
                            (None, false) => "cancelled".to_string(),
                        };
                        format!("{} ({}): {answer}", pick.kind, pick.request)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }
        other => bail!("get reads text, box, probe, title, focus, count or picks; got {other:?}"),
    }
}

fn find(
    ctx: &Ctx,
    by: &str,
    value: &str,
    name: Option<&str>,
    action: Option<&str>,
    action_arg: Option<String>,
) -> anyhow::Result<()> {
    let (mut client, window) = connect(ctx)?;
    let tree = client.tree(window)?;
    let mut snapshot = snapshot::build(
        &tree,
        &Options {
            all: true,
            ..Default::default()
        },
    );
    assign_refs(ctx, &mut snapshot)?;
    let matches: Vec<&snapshot::Entry> = snapshot
        .entries
        .iter()
        .filter(|entry| match by {
            "text" => entry.name.contains(value),
            "role" => entry.role == value && name.is_none_or(|n| entry.name.contains(n)),
            "id" => entry.key.path == value || entry.key.path.ends_with(&format!("/{value}")),
            "source" => entry.source.as_deref().is_some_and(|s| s.contains(value)),
            _ => false,
        })
        .collect();
    if !matches!(by, "text" | "role" | "id" | "source") {
        bail!("find by text, role, id or source; got {by:?}");
    }
    let Some(action) = action else {
        let list: Vec<String> = matches
            .iter()
            .map(|e| {
                let hidden = if e.visible.is_empty() {
                    " [hidden]"
                } else {
                    ""
                };
                format!("@{} {} {:?}{hidden}", e.reference, e.role, e.name)
            })
            .collect();
        return say(ctx, &matches, || {
            if list.is_empty() {
                "no matches".into()
            } else {
                list.join("\n")
            }
        });
    };
    let first = matches
        .first()
        .ok_or_else(|| anyhow!("nothing matches {by} {value:?}"))?;
    let target = format!("@{}", first.reference);
    match action {
        "click" => pointer(
            ctx,
            &target,
            |x, y| click_events(x, y, Button::Left, 1),
            "clicked",
        ),
        "dblclick" => pointer(
            ctx,
            &target,
            |x, y| {
                let mut events = click_events(x, y, Button::Left, 1);
                events.extend(click_events(x, y, Button::Left, 2).into_iter().skip(1));
                events
            },
            "double-clicked",
        ),
        "hover" => pointer(
            ctx,
            &target,
            |x, y| {
                vec![InputEvent::MouseMove {
                    x,
                    y,
                    pressed: None,
                    modifiers: Modifiers::default(),
                }]
            },
            "hovered",
        ),
        "fill" => fill(
            ctx,
            &target,
            &action_arg.ok_or_else(|| anyhow!("fill needs a text"))?,
        ),
        "type" => {
            pointer(
                ctx,
                &target,
                |x, y| click_events(x, y, Button::Left, 1),
                "clicked",
            )?;
            let text = action_arg.ok_or_else(|| anyhow!("type needs a text"))?;
            client.tree(window)?;
            send(
                &mut client,
                window,
                vec![InputEvent::Text { text, keys: false }],
            )?;
            Ok(())
        }
        other => bail!("find acts with click, dblclick, hover, fill or type; got {other:?}"),
    }
}

fn doctor(ctx: &Ctx) -> anyhow::Result<()> {
    let mut failed = false;
    let mut line = |ok: bool, text: String| {
        failed |= !ok;
        println!("{} {text}", if ok { "ok  " } else { "FAIL" });
    };
    line(
        true,
        format!(
            "session {:?} at {}",
            ctx.session.name,
            ctx.session.dir.display()
        ),
    );
    if cfg!(target_os = "linux") {
        let xvfb = which("Xvfb");
        line(
            xvfb.is_some(),
            match xvfb {
                Some(path) => format!("Xvfb at {path}"),
                None => "Xvfb is missing; headless runs need it (apt-get install xvfb)".into(),
            },
        );
    }
    let socket = ctx.session.socket_path();
    match Client::connect(&socket) {
        Err(error) => {
            line(
                false,
                format!(
                    "no app answers on {} ({error}); `gpui-agent launch -- <command>` starts one",
                    socket.display()
                ),
            );
            if let Some(pid) = ctx.session.pid("app.pid") {
                line(
                    false,
                    format!(
                        "pid {pid} was launched; last lines of its log:\n{}",
                        session::tail(&ctx.session.file("app.log"), 10)
                    ),
                );
            }
        }
        Ok(mut client) => {
            match client.call::<gpui_agent_protocol::Hello>(&Request::Hello) {
                Ok(hello) => line(
                    hello.protocol == gpui_agent_protocol::PROTOCOL_VERSION,
                    format!("app pid {} speaks protocol {}", hello.pid, hello.protocol),
                ),
                Err(error) => line(false, format!("hello failed: {error:#}")),
            }
            match ctx.session.window(&mut client, ctx.window) {
                Err(error) => line(false, format!("{error:#}")),
                Ok(window) => {
                    match client.tree(window) {
                        Ok(tree) => line(
                            !tree.nodes.is_empty(),
                            format!(
                                "window {window} records {} elements and {} lines of text",
                                tree.nodes.len(),
                                tree.texts.len()
                            ),
                        ),
                        Err(error) => line(false, format!("frame record: {error:#}")),
                    }
                    let path = ctx.session.file("doctor.png");
                    match client.call::<ScreenshotInfo>(&Request::Screenshot {
                        window,
                        path: path.to_string_lossy().to_string(),
                    }) {
                        Ok(info) => {
                            line(true, format!("screenshot {}x{}", info.width, info.height))
                        }
                        Err(error) => line(false, format!("screenshot: {error:#}")),
                    }
                }
            }
        }
    }
    if failed {
        bail!("some checks failed");
    }
    Ok(())
}

fn which(program: &str) -> Option<String> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|path| path.is_file())
            .map(|path| path.to_string_lossy().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut current = String::new();
        let mut quote = None;
        let mut started = false;
        for c in line.chars() {
            match (quote, c) {
                (None, '#') if !started || current.is_empty() => break,
                (None, '\'' | '"') => {
                    quote = Some(c);
                    started = true;
                }
                (Some(q), c) if c == q => quote = None,
                (None, ' ') => {
                    if started {
                        out.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                (_, c) => {
                    current.push(c);
                    started = true;
                }
            }
        }
        if started {
            out.push(current);
        }
        out
    }

    const README: &str = include_str!("../../../README.md");

    fn commands_parse(doc: &str, name: &str) -> usize {
        let mut in_block = false;
        let mut checked = 0;
        for line in doc.lines() {
            if line.starts_with("```") {
                in_block = line.trim() == "```bash";
                continue;
            }
            let line = line.trim();
            if !in_block || !line.starts_with("gpui-agent ") {
                continue;
            }
            let args = words(line);
            if let Err(error) = Cli::try_parse_from(&args) {
                use clap::error::ErrorKind::{DisplayHelp, DisplayVersion};
                if !matches!(error.kind(), DisplayHelp | DisplayVersion) {
                    panic!("{name} shows a command that does not parse: {line}\n{error}");
                }
            }
            checked += 1;
        }
        checked
    }

    #[test]
    fn every_command_in_the_guide_parses() {
        let checked = commands_parse(CORE_SKILL, "the guide");
        assert!(checked > 40, "only {checked} commands found in the guide");
    }

    #[test]
    fn every_command_in_the_readme_parses() {
        let checked = commands_parse(README, "the README");
        assert!(checked > 60, "only {checked} commands found in the README");
    }
}
