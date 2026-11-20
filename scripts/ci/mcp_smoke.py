"""Drives a running `voxferro --mcp` the way an agent would, and fails on
anything unexpected. Standard library only.

Usage: python scripts/ci/mcp_smoke.py [url]   (default http://127.0.0.1:47800/mcp)

Waits for the server, then, with a session as clients did up to protocol
2025-11-25: initialize, list the tools, describe the scene, build something
with run_commands (one undo step), undo it, and capture the view as a PNG.
Then without one, as 2026-07-28 clients do (no initialize, the version on
each request): discover the server, check the tools' annotations and
describe_scene's structured content, and render with a progress token,
which must bring progress notifications before the image."""
import base64, json, sys, time, urllib.error, urllib.request

URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:47800/mcp"
STATELESS = "2026-07-28"
session = None
stateless = False
next_id = 0


def post(message):
    """Sends one message; returns its reply and the notifications that came
    before it on the same stream."""
    global session
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if stateless:
        headers["MCP-Protocol-Version"] = STATELESS
        headers["Mcp-Method"] = message["method"]
        if message["method"] == "tools/call":
            headers["Mcp-Name"] = message["params"]["name"]
        meta = message.setdefault("params", {}).setdefault("_meta", {})
        meta["io.modelcontextprotocol/protocolVersion"] = STATELESS
        meta["io.modelcontextprotocol/clientCapabilities"] = {}
        meta["io.modelcontextprotocol/clientInfo"] = {"name": "ci", "version": "0"}
    elif session:
        headers["Mcp-Session-Id"] = session
    request = urllib.request.Request(URL, data=json.dumps(message).encode(), headers=headers)
    with urllib.request.urlopen(request, timeout=300) as response:
        if stateless and response.headers.get("Mcp-Session-Id"):
            sys.exit("a stateless request was given a session")
        session = response.headers.get("Mcp-Session-Id") or session
        body = response.read().decode()
    if not body.strip():
        return None, []
    if body.lstrip().startswith("{"):
        return json.loads(body), []
    notes = []
    for line in body.splitlines():
        if line.startswith("data:") and line[5:].strip():
            event = json.loads(line[5:])
            if event.get("id") == message.get("id") and "method" not in event:
                return event, notes
            notes.append(event)
    return None, notes


def rpc(method, params, with_notes=False):
    global next_id
    next_id += 1
    reply, notes = post({"jsonrpc": "2.0", "id": next_id, "method": method, "params": params})
    if reply is None or "error" in reply:
        sys.exit(f"{method}: {reply}")
    return (reply["result"], notes) if with_notes else reply["result"]


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
print(f"ok with a session: {len(tools)} tools, batch of {voxels} voxels undone in one step, "
      f"{len(png)}-byte capture")

# The stateless protocol: no initialize, no session.
stateless = True
found = rpc("server/discover", {})
if STATELESS not in found.get("supportedVersions", []):
    sys.exit(f"server/discover does not offer {STATELESS}: {found}")
listed = {t["name"]: t for t in rpc("tools/list", {})["tools"]}
for name, hint in [("describe_scene", "readOnlyHint"), ("get_palette", "readOnlyHint"),
                   ("capture_view", "readOnlyHint"), ("save_file", "destructiveHint"),
                   ("run_command", "destructiveHint")]:
    if not listed[name].get("annotations", {}).get(hint):
        sys.exit(f"{name} lacks {hint}: {listed[name].get('annotations')}")
if "outputSchema" not in listed["describe_scene"]:
    sys.exit("describe_scene has no output schema")

call("run_commands", {"commands": ["new 16", "box 2 2 0 13 13 3 12"]})
scene = rpc("tools/call", {"name": "describe_scene", "arguments": {}})
structured = scene.get("structuredContent") or {}
if sum(m["voxels"] for m in structured.get("models", [])) != 12 * 12 * 4:
    sys.exit(f"describe_scene's structured content is wrong: {structured}")

samples = 4
result, notes = rpc("tools/call", {"name": "capture_view",
                                   "arguments": {"render": True, "samples": samples, "max_size": 128},
                                   "_meta": {"progressToken": "render"}}, with_notes=True)
if result.get("isError"):
    sys.exit(f"render capture failed: {result}")
progress = [n["params"] for n in notes if n.get("method") == "notifications/progress"]
if not progress or any(p.get("total") != samples or p.get("progressToken") != "render" for p in progress):
    sys.exit(f"expected progress notifications out of {samples} samples, got {notes}")
png = base64.b64decode(result["content"][0]["data"])
if png[:8] != b"\x89PNG\r\n\x1a\n":
    sys.exit("the render capture did not return a PNG")
print(f"ok without a session ({STATELESS}): annotations, structured content, "
      f"{len(progress)} progress notifications before a {len(png)}-byte render")
