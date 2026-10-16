"""Drives a running `voxferro --mcp` the way an agent would, and fails on
anything unexpected. Standard library only.

Usage: python scripts/ci/mcp_smoke.py [url]   (default http://127.0.0.1:47800/mcp)

Waits for the server, then: initialize, list the tools, describe the scene,
build something with run_commands (one undo step), undo it, and capture the
view as a PNG."""
import base64, json, sys, time, urllib.error, urllib.request

URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:47800/mcp"
session = None
next_id = 0


def post(message):
    global session
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if session:
        headers["Mcp-Session-Id"] = session
    request = urllib.request.Request(URL, data=json.dumps(message).encode(), headers=headers)
    with urllib.request.urlopen(request, timeout=300) as response:
        session = response.headers.get("Mcp-Session-Id") or session
        body = response.read().decode()
    if not body.strip():
        return None
    if body.lstrip().startswith("{"):
        return json.loads(body)
    for line in body.splitlines():
        if line.startswith("data:") and line[5:].strip():
            reply = json.loads(line[5:])
            if reply.get("id") == message.get("id"):
                return reply
    return None


def rpc(method, params):
    global next_id
    next_id += 1
    reply = post({"jsonrpc": "2.0", "id": next_id, "method": method, "params": params})
    if reply is None or "error" in reply:
        sys.exit(f"{method}: {reply}")
    return reply["result"]


def call(tool, arguments=None):
    result = rpc("tools/call", {"name": tool, "arguments": arguments or {}})
    if result.get("isError"):
        sys.exit(f"{tool} failed: {result}")
    return result["content"]


for attempt in range(60):
    try:
        info = rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                  "clientInfo": {"name": "ci", "version": "0"}})
        break
    except (urllib.error.URLError, ConnectionError):
        time.sleep(2)
else:
    sys.exit(f"no MCP server at {URL}")
print("server:", info["serverInfo"])
post({"jsonrpc": "2.0", "method": "notifications/initialized"})

tools = {t["name"] for t in rpc("tools/list", {})["tools"]}
expected = {"run_command", "run_commands", "describe_scene", "get_palette", "set_voxels",
            "open_file", "save_file", "capture_view"}
if not expected <= tools:
    sys.exit(f"missing tools: {expected - tools}")

before = json.loads(call("describe_scene")[0]["text"])
call("run_commands", {"commands": ["new 16", "box 2 2 0 13 13 3 12", "voxels 8 8 4 40"]})
after = json.loads(call("describe_scene")[0]["text"])
voxels = sum(m["voxels"] for m in after["models"])
if voxels != 12 * 12 * 4 + 1:
    sys.exit(f"expected {12 * 12 * 4 + 1} voxels, got {voxels}")
call("run_command", {"command": "undo"})
undone = json.loads(call("describe_scene")[0]["text"])
if sum(m["voxels"] for m in undone["models"]) != 0:
    sys.exit("undo did not remove the batch as one step")

image = call("capture_view", {"max_size": 256})[0]
png = base64.b64decode(image["data"])
if image["type"] != "image" or png[:8] != b"\x89PNG\r\n\x1a\n" or len(png) < 500:
    sys.exit("capture_view did not return a PNG")
print(f"ok: {len(tools)} tools, batch of {voxels} voxels undone in one step, {len(png)}-byte capture")
