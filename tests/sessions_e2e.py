#!/usr/bin/env python3
"""Session-only Core integration; no providers, prompts or live user sessions."""
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("KANBAN_BINARY", ROOT / "target/debug/omarchy-kanban"))
FIXTURE = ROOT / "tests/fake_api.py"
FIXTURE.chmod(0o755)


def wait(callback, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            result = callback()
            if result:
                return result
        except (OSError, ValueError, KeyError, StopIteration):
            pass
        time.sleep(0.1)
    raise AssertionError("Condition timed out")


with tempfile.TemporaryDirectory(prefix="session-board-", dir="/tmp/opencode") as temporary:
    folder = Path(temporary)
    fixture = folder / "opencode.json"
    base = int(time.time() * 1000)
    def row(identifier, **extra):
        return {"id": identifier, "title": identifier, "agent": "build", "location": {"directory": "/test/project"}, "time": {"created": base - 100, "updated": base}, **extra}
    data = {"sessions": [row("ses_idle", outcome="succeeded"), row("ses_running"), row("ses_waiting"), row("ses_parent"), row("ses_child", parentID="ses_parent"), row("ses_archived", time={"created": base, "updated": base, "archived": base})], "active": ["ses_running", "ses_waiting", "ses_child"], "permission": {"ses_waiting": [{"id": "permission", "action": "shell"}]}, "form": {"ses_waiting": [{"id": "form", "title": "Choose deployment"}]}}
    def save():
        temporary = fixture.with_suffix(".new")
        temporary.write_text(json.dumps(data))
        temporary.replace(fixture)
    save()
    listener = socket.socket(); listener.bind(("127.0.0.1", 0)); port = listener.getsockname()[1]; listener.close()
    environment = {**os.environ, "KANBAN_FIXTURE": str(fixture), "KANBAN_FAKE_API_LOG": str(folder / "reads.jsonl"), "XDG_STATE_HOME": str(folder / "state")}
    token = ""; core_id = ""; process = None; bridge = None
    def start():
        return subprocess.Popen([BINARY, "serve", "--data-dir", folder / "core", "--port", str(port), "--opencode", FIXTURE], stdout=subprocess.DEVNULL, stderr=open(folder / "core.log", "ab"), env=environment)
    def api(body=None, auth=True):
        request = urllib.request.Request(f"http://127.0.0.1:{port}/api/commands" if body else f"http://127.0.0.1:{port}/api/state", data=json.dumps(body).encode() if body else None, headers={"content-type": "application/json", "x-core-id": core_id, **({"Authorization": "Bearer " + token} if auth else {})})
        with urllib.request.urlopen(request, timeout=35) as response:
            return json.load(response)
    def session(identifier):
        return next(s for s in api()["sessions"] if s["id"] == identifier)
    def command(identifier, action="session_done", command_id=None, updated=None):
        return {"command_id": command_id or str(uuid.uuid4()), "action": action, "id": identifier, "observed_updated": session(identifier)["updated"] if updated is None else updated}
    try:
        # Arbitrary old runner data must remain byte-for-byte untouched.
        (folder / "core").mkdir()
        legacy = folder / "core/kanban.sqlite3"
        legacy.write_bytes(b"preserve previous experimental task database")
        old_hash = hashlib.sha256(legacy.read_bytes()).digest()
        process = start(); wait(lambda: (folder / "core/token").exists()); token = (folder / "core/token").read_text()
        board = wait(lambda: (s if (s := api())["connected"] else None)); core_id = board["core_id"]
        assert board["mode"] == "sessions" and board["protocol"] == 2
        assert len(board["sessions"]) == 4
        assert session("ses_idle")["status"] == "open"  # outcome=succeeded is NOT DONE
        assert session("ses_running")["status"] == "running"
        assert session("ses_waiting")["status"] == "waiting" and session("ses_waiting")["pending_count"] == 2
        assert session("ses_parent")["active_children"] == 1
        try:
            api(auth=False); raise AssertionError("Unauthenticated access")
        except urllib.error.HTTPError as e:
            assert e.code == 401
        done = command("ses_idle", command_id="done-once")
        assert api(done)["status"] == "done"
        assert api(done)["status"] == "done"
        wait(lambda: session("ses_idle")["status"] == "done")
        for identifier in ("ses_running", "ses_waiting", "ses_parent"):
            try:
                api(command(identifier)); raise AssertionError("Hidden active work")
            except urllib.error.HTTPError:
                pass
        api(command("ses_idle", "session_reopen")); assert session("ses_idle")["status"] == "open"
        api(command("ses_idle"))
        process.send_signal(signal.SIGINT); process.wait(timeout=5); process = start()
        wait(lambda: api()["connected"]); assert session("ses_idle")["status"] == "done"
        data["active"].append("ses_idle"); save()
        wait(lambda: session("ses_idle")["status"] == "running")
        data["active"].remove("ses_idle"); save()
        wait(lambda: session("ses_idle")["status"] == "open")
        api(command("ses_idle"))
        data["sessions"][0]["time"]["updated"] += 10; save()
        wait(lambda: session("ses_idle")["status"] == "open")
        try:
            api(command("ses_idle", updated=base)); raise AssertionError("Stale card accepted")
        except urllib.error.HTTPError:
            pass
        assert hashlib.sha256(legacy.read_bytes()).digest() == old_hash
        profile = folder / "profile.json"; profile.write_text(json.dumps({"core_id": core_id, "token": token, "port": port, "ssh": None}))
        bridge = subprocess.Popen([BINARY, "bridge", "--profile", profile], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1, env=environment)
        def bridge_online():
            if select.select([bridge.stdout], [], [], 0.1)[0]:
                value = json.loads(bridge.stdout.readline()); return value if value["opencode_connected"] else None
        assert len(wait(bridge_online)["sessions"]) == 4
        data["offline"] = True; save()
        wait(lambda: not api()["connected"])
        assert len(api()["sessions"]) == 4  # stale cards, not an empty success
        try:
            api(command("ses_idle")); raise AssertionError("Offline DONE accepted")
        except urllib.error.HTTPError:
            pass
        requests = [json.loads(line) for line in (folder / "reads.jsonl").read_text().splitlines()]
        assert requests and all(r[:2] == ["api", "GET"] for r in requests)
        print("PASS: pagination, real-status mapping, parent/child activity, questions/permissions, manual DONE/reopen, persistence, auto-reopen, stale/offline rejection, bridge and untouched legacy DB. All OpenCode calls were GET.")
    finally:
        if bridge:
            bridge.terminate(); bridge.wait(timeout=5)
        if process and process.poll() is None:
            process.send_signal(signal.SIGINT)
            try: process.wait(timeout=5)
            except subprocess.TimeoutExpired: process.kill()
