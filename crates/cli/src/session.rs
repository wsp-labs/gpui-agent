//! A session is one app the command line drives: its socket, its process, its Xvfb and its
//! last snapshot, kept in a folder of its own so parallel agents never share state.

use anyhow::{Context as _, anyhow, bail};
use gpui_agent_protocol::{
    ErrorKind, ErrorReply, Hello, PROTOCOL_VERSION, RawTree, Request, Response, SOCKET_ENV,
    WindowInfo,
};
use serde::de::DeserializeOwned;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Session {
    pub name: String,
    pub dir: PathBuf,
}

#[derive(Debug)]
pub struct AppError(pub ErrorReply);

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.message)
    }
}

impl std::error::Error for AppError {}

pub fn state_root() -> PathBuf {
    gpui_agent_protocol::state_root()
}

impl Session {
    pub fn open(name: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'),
            "session names use letters, digits, '-', '_' and '.'"
        );
        let dir = state_root().join(name);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Session {
            name: name.into(),
            dir,
        })
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.file(name))
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    }

    pub fn write(&self, name: &str, text: &str) -> anyhow::Result<()> {
        std::fs::write(self.file(name), text).with_context(|| format!("write {name}"))
    }

    pub fn socket_path(&self) -> PathBuf {
        self.read("socket")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.file("app.sock"))
    }

    pub fn pid(&self, name: &str) -> Option<i32> {
        self.read(name)?.parse().ok()
    }

    pub fn connect(&self) -> anyhow::Result<Client> {
        let path = self.socket_path();
        Client::connect(&path).with_context(|| {
            format!(
                "no app in session {:?}; start one with `gpui-agent launch -- <command>`",
                self.name
            )
        })
    }

    pub fn window(&self, client: &mut Client, choice: Option<u64>) -> anyhow::Result<u64> {
        let windows: Vec<WindowInfo> = client.call(&Request::Windows)?;
        anyhow::ensure!(!windows.is_empty(), "the app has no open windows");
        let wanted = choice.or_else(|| self.read("window").and_then(|w| w.parse().ok()));
        if let Some(wanted) = wanted {
            if windows.iter().any(|w| w.window == wanted) {
                return Ok(wanted);
            }
            if choice.is_some() {
                bail!("no window {wanted}; `gpui-agent windows` lists them");
            }
        }
        let pick = windows
            .iter()
            .find(|w| w.active)
            .unwrap_or(&windows[0])
            .window;
        Ok(pick)
    }
}

pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(path: &Path) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;
        Ok(Client {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        })
    }

    pub fn raw(&mut self, request: &Request) -> anyhow::Result<Response> {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        let mut reply = String::new();
        let read = self.reader.read_line(&mut reply)?;
        anyhow::ensure!(read > 0, "the app closed the connection");
        Ok(serde_json::from_str(&reply)?)
    }

    pub fn call<T: DeserializeOwned>(&mut self, request: &Request) -> anyhow::Result<T> {
        match self.raw(request)? {
            Response::Ok { ok } => Ok(serde_json::from_value(ok)?),
            Response::Err { error } => Err(AppError(error).into()),
        }
    }

    pub fn tree(&mut self, window: u64) -> anyhow::Result<RawTree> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.call::<RawTree>(&Request::Tree { window }) {
                Err(error)
                    if error
                        .downcast_ref::<AppError>()
                        .is_some_and(|e| e.0.kind == ErrorKind::Warming) =>
                {
                    if Instant::now() > deadline {
                        bail!(
                            "window {window} drew no frame for 5s. A hidden or minimized window draws none on a Mac; bring it on screen"
                        );
                    }
                    std::thread::sleep(Duration::from_millis(30));
                }
                other => return other,
            }
        }
    }
}

pub struct Launch {
    pub command: Vec<String>,
    pub headless: bool,
    pub screen: String,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
}

fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn free_display() -> anyhow::Result<u32> {
    (90..400)
        .find(|n| {
            !Path::new(&format!("/tmp/.X11-unix/X{n}")).exists()
                && !Path::new(&format!("/tmp/.X{n}-lock")).exists()
        })
        .ok_or_else(|| anyhow!("no free X display between :90 and :399"))
}

fn start_xvfb(session: &Session, screen: &str) -> anyhow::Result<String> {
    let display = free_display()?;
    let log = std::fs::File::create(session.file("xvfb.log"))?;
    let child = Command::new("Xvfb")
        .arg(format!(":{display}"))
        .args(["-screen", "0", screen, "-nolisten", "tcp"])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .context("start Xvfb (install it with `apt-get install xvfb`)")?;
    session.write("xvfb.pid", &child.id().to_string())?;
    let socket = format!("/tmp/.X11-unix/X{display}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !Path::new(&socket).exists() {
        anyhow::ensure!(
            Instant::now() < deadline,
            "Xvfb did not start; see {}",
            session.file("xvfb.log").display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(format!(":{display}"))
}

pub fn launch(session: &Session, launch: Launch) -> anyhow::Result<(Hello, u32, Vec<WindowInfo>)> {
    if let Some(pid) = session.pid("app.pid")
        && alive(pid)
    {
        bail!(
            "session {:?} already runs an app (pid {pid}); `gpui-agent close` stops it",
            session.name
        );
    }
    close(session).ok();

    let (program, args) = launch
        .command
        .split_first()
        .ok_or_else(|| anyhow!("give the app's command after `--`"))?;
    let socket = session.file("app.sock");
    std::fs::remove_file(&socket).ok();
    session.write("socket", &socket.to_string_lossy())?;

    let mut command = Command::new(program);
    command.args(args).env(SOCKET_ENV, &socket);
    for (key, value) in &launch.env {
        command.env(key, value);
    }
    let on_linux = cfg!(target_os = "linux");
    let no_display =
        std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none();
    if launch.headless || (on_linux && no_display) {
        anyhow::ensure!(
            on_linux,
            "headless runs need Linux; on this computer the window opens on screen"
        );
        let display = start_xvfb(session, &launch.screen)?;
        command
            .env("DISPLAY", &display)
            .env_remove("WAYLAND_DISPLAY");
        session.write("display", &display)?;
    }
    let log = std::fs::File::create(session.file("app.log"))?;
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    unsafe {
        use std::os::unix::process::CommandExt as _;
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = command
        .spawn()
        .with_context(|| format!("start {program}"))?;
    let pid = child.id();
    session.write("app.pid", &pid.to_string())?;
    std::fs::remove_file(session.file("window")).ok();
    std::fs::remove_file(session.file("refs.json")).ok();

    let deadline = Instant::now() + launch.timeout;
    let mut client = loop {
        if !alive(pid as i32) {
            bail!(
                "the app exited before it answered; last lines of {}:\n{}",
                session.file("app.log").display(),
                tail(&session.file("app.log"), 20)
            );
        }
        if let Ok(client) = Client::connect(&socket) {
            break client;
        }
        if Instant::now() > deadline {
            bail!(
                "the app did not open {} within {:?}. Does it call gpui_agent::init(cx)? Last lines of its log:\n{}",
                socket.display(),
                launch.timeout,
                tail(&session.file("app.log"), 20)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let hello: Hello = client.call(&Request::Hello)?;
    anyhow::ensure!(
        hello.protocol == PROTOCOL_VERSION,
        "the app speaks protocol {} and this command line speaks {PROTOCOL_VERSION}; build both from the same gpui-agent",
        hello.protocol
    );
    let windows = loop {
        let windows: Vec<WindowInfo> = client.call(&Request::Windows)?;
        if !windows.is_empty() || Instant::now() > deadline {
            break windows;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    Ok((hello, pid, windows))
}

fn stop(pid: i32) {
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
        libc::kill(pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    if alive(pid) {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
            libc::kill(pid, libc::SIGKILL);
        }
    }
}

pub fn close(session: &Session) -> anyhow::Result<bool> {
    let mut stopped = false;
    for name in ["app.pid", "xvfb.pid"] {
        if let Some(pid) = session.pid(name)
            && alive(pid)
        {
            stop(pid);
            stopped = true;
        }
    }
    for name in [
        "app.pid",
        "xvfb.pid",
        "app.sock",
        "socket",
        "display",
        "window",
        "refs.json",
    ] {
        std::fs::remove_file(session.file(name)).ok();
    }
    Ok(stopped)
}

pub fn tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}
