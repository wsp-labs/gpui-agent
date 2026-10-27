#!/usr/bin/env python3
"""Drives `gpui-agent mcp` over stdio like an MCP client and checks each reply.

Usage: scripts/mcp_e2e.py <dir holding gpui-agent and gpui-agent-testapp>
"""

import base64
import json
import os
import subprocess
import sys
import tempfile

bin_dir = os.path.abspath(sys.argv[1])
env = dict(os.environ)
env["PATH"] = bin_dir + os.pathsep + env["PATH"]
env["GPUI_AGENT_HOME"] = tempfile.mkdtemp()
env.pop("DISPLAY", None)
env.pop("WAYLAND_DISPLAY", None)

server = subprocess.Popen(
    [os.path.join(bin_dir, "gpui-agent"), "--session", "mcp-e2e", "mcp"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True,
    env=env,
)
next_id = 0
checks = 0


def call(method, params=None):
    global next_id
    next_id += 1
    server.stdin.write(json.dumps({"jsonrpc": "2.0", "id": next_id, "method": method, "params": params or {}}) + "\n")
    server.stdin.flush()
    reply = json.loads(server.stdout.readline())
    assert reply["id"] == next_id, reply
    return reply


def tool(name, **arguments):
    reply = call("tools/call", {"name": name, "arguments": arguments})
    return reply["result"]


def text_of(result):
    return "\n".join(c["text"] for c in result["content"] if c["type"] == "text")


def check(condition, message):
    global checks
    if not condition:
        print("FAIL:", message)
        sys.exit(1)
    checks += 1
    print("ok  ", message)


try:
    init = call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "0"}})
    check(init["result"]["serverInfo"]["name"] == "gpui-agent", "initialize names the server")
    server.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
    server.stdin.flush()

    names = {t["name"] for t in call("tools/list")["result"]["tools"]}
    check({"launch", "snapshot", "click", "fill", "screenshot", "record", "apps", "wait", "find", "action"} <= names, f"tools/list has the core tools ({len(names)} tools)")

    launched = tool("launch", command=["gpui-agent-testapp"])
    check(not launched.get("isError") and "launched pid" in text_of(launched), "launch starts the app")

    snap = text_of(tool("snapshot", interactive=True))
    check('@e4 button "Increment"' in snap, "snapshot lists the Increment button as @e4")
    check('button "Settings"' in snap, "an accessible label names the icon button")

    clicked = tool("click", target="@e4")
    check("clicked @e4" in text_of(clicked), "click by ref")

    count = text_of(tool("get", what="probe", arg="count"))
    check(count.strip() == "1", f"probe count is 1 (got {count.strip()!r})")

    filled = tool("fill", target="@e6", text="Ada")
    check(not filled.get("isError"), "fill the text field")
    waited = tool("wait", text="hello, Ada", timeout=3000)
    check(not waited.get("isError"), "wait for the greeting")

    hidden = tool("click", target="text:row 40")
    check(hidden.get("isError") and "not visible" in text_of(hidden), "a hidden row is refused as an error result")
    tool("scroll", into_view="text:row 40")
    check("clicked" in text_of(tool("click", target="text:row 40")), "scroll into view, then click")

    shot = tool("screenshot", annotate=True)
    images = [c for c in shot["content"] if c["type"] == "image"]
    check(len(images) == 1 and base64.b64decode(images[0]["data"])[:8] == b"\x89PNG\r\n\x1a\n", "screenshot returns a PNG inline")
    check("[4] @e4" in text_of(shot), "annotated screenshot carries its legend")

    element = tool("screenshot", element="id:increment")
    check(any(c["type"] == "image" for c in element["content"]), "screenshot of one element")

    started = tool("record", action="start", fps=10)
    check(not started.get("isError"), "record starts")
    tool("click", target="id:increment")
    tool("wait", ms=500)
    stopped = tool("record", action="stop", path=os.path.join(env["GPUI_AGENT_HOME"], "rec.gif"))
    check("frames" in text_of(stopped), f"record stops with frames ({text_of(stopped).strip()})")

    missing = call("tools/call", {"name": "nope", "arguments": {}})
    check("error" in missing, "an unknown tool is a protocol error")

    closed = tool("close")
    check("closed" in text_of(closed), "close stops the app")
    print(f"\nall {checks} checks passed")
finally:
    subprocess.run([os.path.join(bin_dir, "gpui-agent"), "--session", "mcp-e2e", "close"], env=env, capture_output=True)
    server.stdin.close()
    server.wait(timeout=10)
