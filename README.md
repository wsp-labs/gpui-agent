# gpui-agent

Lets a coding agent see and drive a GPUI app the way Playwright drives a browser: read what is on
screen as text, click, type, scroll, wait and take screenshots, from the command line.

```
$ gpui-agent launch -- ./target/debug/my-app
launched pid 48211 on Xvfb :90
window 4294967297 "" 480x520

$ gpui-agent snapshot -i
window 4294967297 "" 480x520
  @e1 group "root" ctx=TestApp [focused]  crates/testapp/src/main.rs:55
    @e2 button "Increment"  crates/testapp/src/main.rs:42
    @e4 textbox "Your name" ctx=TextInput  crates/testapp/src/input.rs:632
    @e6 scroll "rows"  crates/testapp/src/main.rs:111
      @e7 button "row 1"  crates/testapp/src/main.rs:118
      @e13 button "row 7" [clipped]  crates/testapp/src/main.rs:118

$ gpui-agent click @e2
clicked @e2 button "Increment"

$ gpui-agent fill @e4 "Ada"
filled @e4 with "Ada"
```

Every line carries the source location that built the element, so an agent goes from what it
sees straight to the code that drew it.

## How it works

- **`patches/gpui`**: five changes to Zed's GPUI. While a window flag is on, GPUI records each
  frame's elements (bounds, element id, source location, key context, what they respond to), the
  lines of text each element painted, and the focus handles that registered a text input. Linux
  windows get `render_to_image`, which macOS and Windows already had. A hook lets a tool answer
  open and save panels in place of the platform. `BASE` names the Zed commit they apply to.
- **`patches/gpui-pre-0.3.9`**: the same changes for crates.io's `gpui-pre 0.3.9`, the GPUI that
  gpui-kit pins, one patch per published crate.
- **`crates/gpui-agent`**: the crate an app calls at startup. It does nothing unless the app was
  started with `GPUI_AGENT_SOCKET` set. Then it answers requests on that Unix socket from the
  main thread: windows, the frame record, input, screenshots, actions and probes.
- **`crates/cli`**: the `gpui-agent` command. It starts the app (under a private Xvfb on Linux
  when there is no display), builds the outline from the frame record, keeps refs stable across
  snapshots, refuses clicks on hidden or covered elements, and waits for conditions.
- **`gpui-agent mcp`**: the same commands as MCP tools over stdio. Each tool call runs the
  command, so the two cannot drift apart, and `screenshot` returns the image inline.
- **`crates/protocol`**: the messages between the two, one JSON object per line.
- **`crates/testapp`**: a GPUI app with buttons, a real text field, a long scrolling list, a cached
  view, an overlay that covers buttons, actions and probes. `scripts/e2e.sh` drives it through
  every command.

## Using it in an app

1. Build against a patched GPUI.
   - On Zed's GPUI: check out Zed at the commit in `patches/gpui/BASE` and run
     `git am patches/gpui/*.patch`.
   - On gpui-kit or `gpui-pre 0.3.9`: unpack `gpui-pre`, `gpui-pre-wgpu` and `gpui-pre-linux`
     0.3.9, apply each crate's patch from `patches/gpui-pre-0.3.9`, and point
     `[patch.crates-io]` at them. `scripts/gpui-pre.sh` does exactly this.
2. Add `gpui-agent` as a dependency with the feature for your GPUI: `zed` or `gpui-pre-0-3-9`.
   Enable `test-support` on GPUI and on `gpui_platform` in the builds you drive; screenshots need
   it.
3. Call `gpui_agent::init(cx)` once in `Application::run`.
4. Optionally expose state with `gpui_agent::probe(cx, "name", |cx| json)`.

Then `gpui-agent skills get core` prints the full usage guide for the installed version.

## Status

`scripts/e2e.sh` (77 checks) and `scripts/mcp_e2e.py` (14) pass on Linux under Xvfb against Zed's
GPUI, against `gpui-pre 0.3.9`, and against `gpui-pre 0.3.9` carrying another app's own GPUI
patches. On macOS with a real Metal window at 2x they passed against Zed's GPUI; the `gpui-pre`
build compiles on macOS but has not been run there yet. Windows is not supported, since the socket
is a Unix socket.

Known gaps:

- Under the tool, `window.prompt` draws inside the window (GPUI's fallback renderer) so it can be
  read and answered, and open and save panels are answered from `gpui-agent pick`.
- GPUI's fallback prompt does not block clicks to the window behind it. The tool reports
  `[prompt open]` in the snapshot header but does not refuse those clicks, since GPUI delivers them.
- X11 windows report an empty title because GPUI's X11 backend does not implement `get_title`.

## Testing

```bash
cargo test -p gpui-agent-cli
cargo build -p gpui-agent-cli -p gpui-agent-testapp
scripts/e2e.sh target/debug
scripts/mcp_e2e.py target/debug
scripts/e2e.sh "$(scripts/gpui-pre.sh /tmp/gpui-pre)"   # the same against gpui-pre 0.3.9
```

The workspace expects the patched Zed checkout at `../zed`. `rust-toolchain.toml` pins Rust
1.98.1, the version Zed builds with. On a Mac with Homebrew's Rust installed, put
`~/.cargo/bin` first on `PATH`, or Homebrew's older `rustc` builds GPUI and fails.
