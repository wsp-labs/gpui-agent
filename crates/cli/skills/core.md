# gpui-agent core

Drive a GPUI app from the command line: see what is on screen, click, type, wait and take
screenshots. The app must call `gpui_agent::init(cx)` at startup and be built with the patched
GPUI from this repository's `patches/gpui`.

## The loop

```bash
gpui-agent launch -- ./target/debug/my-app   # start the app and wait until it answers
gpui-agent snapshot -i                       # what can be clicked or typed into
gpui-agent click @e3                         # act on a ref from the snapshot
gpui-agent wait --idle                       # let the app settle
gpui-agent snapshot -i                       # refs change after the screen changes
```

Refs (`@e1`, `@e2`, ...) come from the last snapshot. A ref names an element by where it was
built and its id path, not by position, so after a redraw it still finds the same element if it
is on screen. When the element is gone the command fails and says so; take a new snapshot.

On Linux with no display, `launch` starts a private Xvfb for the session. `--headless` forces it.
On a Mac the window opens on screen, and a hidden or minimized window draws no frames.

## Reading the screen

```bash
gpui-agent snapshot            # every element with text or input, with source locations
gpui-agent snapshot -i         # only elements that take input
gpui-agent snapshot -a         # include clipped and scrolled-out elements
gpui-agent snapshot -d 2       # stop at depth 2
gpui-agent snapshot -s @e4     # only the subtree under @e4
gpui-agent snapshot --json     # machine-readable
```

A line looks like:

```
@e3 button "Increment"  crates/app/src/main.rs:88
@e5 textbox "Your name" ctx=TextInput
@e7 scroll "rows" [clipped]
```

The role comes from what the element does: `button` has a click handler, `textbox` registered a
text input handler, `scroll` scrolls, `group` takes focus, keys or raw mouse events, `text` is
plain text. An explicit accessibility role wins. The name is the element's accessible label when
it has one (`.aria_label("Settings")` on an icon button), else the text inside it. States show in
brackets: `[focused]`, `[clipped]`, `[disabled]`, `[checked]`, `[unchecked]`. The path at the end
is where the element was built, so you can go straight to the code.

```bash
gpui-agent get text @e3        # all text inside an element
gpui-agent get box @e3         # its bounds in window pixels
gpui-agent get focus           # the focused element
gpui-agent get count "row"     # visible lines containing a text
gpui-agent get title
gpui-agent get probe count     # app state the app chose to expose, as JSON
gpui-agent probes              # the app's probe names
gpui-agent tree                # the raw frame record as JSON
```

## Acting

```bash
gpui-agent click @e3
gpui-agent click @e3 --right
gpui-agent dblclick @e3
gpui-agent hover @e3
gpui-agent fill @e5 "Ada"      # click, then replace all the field's text
gpui-agent type @e5 " Lovelace"  # click, then insert at the cursor
gpui-agent type "more"         # insert into whatever has focus
gpui-agent type --keys "q"     # one keystroke per character, for elements that read keys
gpui-agent press enter
gpui-agent press secondary-a backspace   # secondary is cmd on a Mac, ctrl elsewhere
gpui-agent scroll down 300 --on @e7
gpui-agent scrollintoview @e40
gpui-agent drag @e3 @e9
```

Targets are `@e3` refs, `id:name` for an element id, or `text:words` for an element named by its
text. `find` does the same search and can act on the first match:

```bash
gpui-agent find text "Reset" click
gpui-agent find role button --name "Show" click
gpui-agent find id increment
gpui-agent find source main.rs:120
```

A click refuses when the element is clipped away (scroll it into view) or when something else
would receive the mouse at its center, such as an overlay. The error names what covers it and
where that was built.

## Prompts

When the tool drives an app, `window.prompt` draws inside the window, so a prompt's buttons show
up in the snapshot and `click` answers them. The snapshot header says `[prompt open]` while one is
waiting. GPUI's in-window prompt does not block clicks to the window behind it, so answer the
prompt before doing anything else.

## File panels

Open and save panels never appear while the tool drives an app. Queue the answer first, then do
what opens the panel. With nothing queued the panel is cancelled, and `get picks` says so.

```bash
gpui-agent pick ./report.txt   # the next open or save panel answers this path
gpui-agent click text:"Open file"
gpui-agent pick                # the next panel is cancelled
gpui-agent get picks           # every panel the app asked for and what it got
```

## Actions

GPUI apps are built on actions. Running one by name is often more reliable than clicking.

```bash
gpui-agent actions                         # actions available at the focused element, with keys
gpui-agent action testapp::Reset
gpui-agent action editor::MoveDown '{"count": 3}'
```

## Waiting

```bash
gpui-agent wait @e3            # until the element is on screen and visible
gpui-agent wait --text "Saved" # until a visible text contains this
gpui-agent wait --gone "Loading"
gpui-agent wait --idle         # until the frame stops changing for 300ms
gpui-agent wait 500            # plain milliseconds, a last resort
```

Waits time out after 25 seconds; `--timeout` takes milliseconds.

## Screenshots

```bash
gpui-agent screenshot                    # into the session folder, path printed
gpui-agent screenshot shot.png
gpui-agent screenshot --annotate map.png # numbered boxes; [N] is @eN, legend printed
gpui-agent screenshot --element @e3 b.png  # just one element
```

`fill` and `type` hand text to the field the way an input method does, so they work whatever
keys the app binds. Elements that read raw keys (a terminal, a game) need `type --keys`.

## Recordings

```bash
gpui-agent record start --fps 10
gpui-agent click @e3
gpui-agent record stop demo.mp4          # or demo.gif; needs ffmpeg
```

Frames come from the app's own renderer, like screenshots, so a recording works on a locked
screen or under Xvfb. Without ffmpeg the frames stay as PNGs and the folder is printed.

The screenshot is rendered by the app's own renderer, so it works on a locked screen or under
Xvfb, and it never includes other windows.

## Sessions

Each session runs one app with its own socket, Xvfb, refs and logs. Parallel agents must use
different sessions.

```bash
gpui-agent --session a launch -- ./app
gpui-agent --session b launch -- ./app
export GPUI_AGENT_SESSION="$(gpui-agent session id --prefix my-app)"   # one per git worktree
gpui-agent session list
gpui-agent close
```

To drive an app you started yourself, start it with `GPUI_AGENT=1`, then:

```bash
gpui-agent list                  # apps that announced themselves
gpui-agent attach my-app         # by name or pid
```

`gpui-agent connect /path/to.sock` drives an app started with `GPUI_AGENT_SOCKET` set.

## MCP

`gpui-agent mcp` serves the same commands as MCP tools over stdio: launch, close, snapshot,
click, hover, fill, type, press, scroll, drag, screenshot, record, wait, get, find, actions,
action, pick, windows, apps and doctor. Each call takes an optional `session`. `screenshot` returns the PNG inline.

```json
{ "mcpServers": { "gpui-agent": { "command": "gpui-agent", "args": ["mcp"] } } }
```

## When something fails

Run `gpui-agent doctor`. It checks Xvfb, the socket, the protocol version, the frame record and
a screenshot, and prints the app's last log lines when it is not answering.

- "is no longer on screen": the element went away or the screen changed. Snapshot again.
- "is covered by": an overlay or popover sits on top. Dismiss it or act on it first.
- "is not visible": it is scrolled out or clipped. `scrollintoview` it.
- A window that is hidden or minimized on a Mac draws no frames, so waits time out.
- Typing goes to the focused element. Click the field first, or use `fill` or `type @ref`.
- "took no text after the click": the element is not a text field, or reads keys; try
  `type --keys`.
