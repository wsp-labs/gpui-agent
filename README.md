# gpui-agent

Lets a coding agent see and drive a GPUI app: read what is on screen, click, type, wait and take screenshots.

## Layout

- `patches/gpui`: the GPUI change that records each frame's elements and text. `BASE` names the Zed commit it applies to.
- `spike`: the GPUI examples that proved input, headless screenshots and the element record. They build inside a Zed checkout as `crates/gpui/examples/*.rs` with `--features test-support,gpui_platform/test-support`.
