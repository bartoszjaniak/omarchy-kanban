#!/usr/bin/env python3
"""Local black-box tests against a real Rust Core + deterministic ACP subprocess."""
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
FIXTURE = ROOT / "tests/fake_opencode.py"
FIXTURE.chmod(0o755)


def wait_for(callback, timeout=15):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = callback()
            if value:
                return value
        except (OSError, ValueError, KeyError) as error:
            last = error
        time.sleep(0.1)
    raise AssertionError(f"Timed out: {last}")


with tempfile.TemporaryDirectory(prefix="kanban-e2e-", dir="/tmp/opencode" if Path("/tmp/opencode").exists() else None) as tmp:
    temp = Path(tmp)
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
    listener.close()
    core = None
    bridges = []
    token = ""
    core_id = ""

    def start():
        return subprocess.Popen([BINARY, "legacy-serve", "--data-dir", temp / "core", "--port", str(port), "--opencode", FIXTURE], stdout=subprocess.DEVNULL, stderr=open(temp / "core.log", "ab"))

    def api(path="/api/state", body=None, auth=True, origin=None):
        headers = {"content-type": "application/json", "x-core-id": core_id}
        if auth:
            headers["Authorization"] = "Bearer " + token
        if origin:
            headers["Origin"] = origin
        request = urllib.request.Request(f"http://127.0.0.1:{port}" + path, data=json.dumps(body).encode() if body else None, headers=headers)
        with urllib.request.urlopen(request, timeout=3) as response:
            return json.load(response)

    def command(action, **extra):
        return api("/api/commands", {"command_id": str(uuid.uuid4()), "action": action, **extra})

    def get_task(identifier):
        return next(t for t in api()["tasks"] if t["id"] == identifier)

    def change(action, identifier, **extra):
        task = get_task(identifier)
        return command(action, id=identifier, revision=task["revision"], **extra)

    def add(prompt, cwd="project", **extra):
        directory = temp / cwd
        directory.mkdir(exist_ok=True)
        task = {"title": prompt, "prompt": prompt, "criteria": "Explicit reviewed result", "cwd": str(directory), "agent": "plan", "timeout_secs": 15, **extra}
        identifier = command("add", task=task)["id"]
        change("ready", identifier)
        return identifier

    try:
        core = start()
        wait_for(lambda: (temp / "core/token").exists())
        token = (temp / "core/token").read_text()
        state = wait_for(api)
        core_id = state["core_id"]
        for auth, origin, status in [(False, None, 401), (True, "https://evil.example", 403)]:
            try:
                api(auth=auth, origin=origin)
                raise AssertionError("Unauthorized request accepted")
            except urllib.error.HTTPError as error:
                assert error.code == status

        cmd = {"command_id": "deduplicate", "action": "add", "task": {"title": "Draft"}}
        assert api("/api/commands", cmd) == api("/api/commands", cmd)
        assert len(api()["tasks"]) == 1
        draft = api()["tasks"][0]
        try:
            change("ready", draft["id"])
            raise AssertionError("Incomplete task accepted")
        except urllib.error.HTTPError:
            pass

        permission = add("TEST_PERMISSION")
        wait_for(lambda: get_task(permission).get("pending"))
        dependent = add("Same resource: must wait")
        independent = add("Independent task", "other-project")
        wait_for(lambda: get_task(independent)["review_ready"])
        assert get_task(dependent)["status"] == "todo"
        decision = get_task(permission)["pending"]
        try:
            change("reply", permission, decision_id=decision["id"], response={"option_id": "invented"})
            raise AssertionError("Unknown permission accepted")
        except urllib.error.HTTPError:
            pass
        change("reply", permission, decision_id=decision["id"], response={"option_id": "once"})
        wait_for(lambda: get_task(permission)["review_ready"])
        change("accept", permission)
        assert get_task(permission)["status"] == "done"
        wait_for(lambda: get_task(dependent)["review_ready"])

        form = add("TEST_FORM")
        decision = wait_for(lambda: get_task(form).get("pending"))
        try:
            change("reply", form, decision_id=decision["id"], response={"action": "accept", "content": {"environment": "invented"}})
            raise AssertionError("Invalid form accepted")
        except urllib.error.HTTPError:
            pass
        change("reply", form, decision_id=decision["id"], response={"action": "accept", "content": {"environment": "test"}})
        wait_for(lambda: get_task(form)["review_ready"])
        change("accept", form)
        change("message", form, text="Follow-up in saved session")
        wait_for(lambda: get_task(form)["review_ready"])
        assert "Test result" in get_task(form)["result"]

        cancel = add("TEST_WAIT", "cancel-project")
        wait_for(lambda: get_task(cancel)["session_id"])
        change("cancel", cancel)
        wait_for(lambda: get_task(cancel)["status"] == "cancelled")
        crash = add("TEST_CRASH", "crash-project")
        wait_for(lambda: get_task(crash)["status"] == "error")
        timeout = add("TEST_WAIT", "timeout-project", timeout_secs=10)
        wait_for(lambda: get_task(timeout)["status"] == "error", timeout=20)
        assert "limit" in get_task(timeout)["error"]
        assert get_task(timeout)["attempts"] == 1
        api("/api/maintenance", {"enabled": True})
        paused = add("Paused task", "maintenance-project")
        time.sleep(1.5)
        assert get_task(paused)["status"] == "todo"
        api("/api/maintenance", {"enabled": False})
        wait_for(lambda: get_task(paused)["review_ready"])

        queued = add("TEST_PERMISSION", "queued-project")
        decision = wait_for(lambda: get_task(queued).get("pending"))
        holders = [add("TEST_WAIT", f"holder-{n}") for n in range(2)]
        wait_for(lambda: all(get_task(identifier)["session_id"] for identifier in holders))
        change("reply", queued, decision_id=decision["id"], response={"option_id": "once"})
        time.sleep(0.5)
        assert get_task(queued)["status"] == "waiting" and get_task(queued)["pending"]["answering"]
        change("cancel", holders[0])
        wait_for(lambda: get_task(queued)["review_ready"])
        change("cancel", holders[1])
        wait_for(lambda: get_task(holders[1])["status"] == "cancelled")

        profile = temp / "profile.json"
        profile.write_text(json.dumps({"ssh": None, "port": port, "token": token, "core_id": core_id}))
        env = {**os.environ, "XDG_STATE_HOME": str(temp / "client-state"), "KANBAN_NO_NOTIFICATIONS": "1"}
        for _ in range(2):
            bridges.append(subprocess.Popen([BINARY, "legacy-bridge", "--profile", profile], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env, bufsize=1))
        def bridge_state(process, predicate):
            def read():
                if select.select([process.stdout], [], [], 0.1)[0]:
                    value = json.loads(process.stdout.readline())
                    return value if predicate(value) else None
            return wait_for(read)
        assert bridge_state(bridges[0], lambda v: v["connected"])["core_id"] == core_id
        assert bridge_state(bridges[1], lambda v: v["connected"])["core_id"] == core_id

        orphan = add("TEST_PERMISSION", "restart-project")
        wait_for(lambda: get_task(orphan).get("pending"))
        core.kill()
        core.wait(timeout=5)
        bridges[0].stdin.write(json.dumps({"action": "add", "title": "Offline draft"}) + "\n")
        bridges[0].stdin.flush()
        offline = bridge_state(bridges[0], lambda v: not v["connected"] and len(v["drafts"]) == 1)
        assert len(offline["tasks"]) >= 5
        assert offline["drafts"][0]["command"]["task"]["title"] == "Offline draft"
        core = start()
        wait_for(api)
        recovered = get_task(orphan)
        assert recovered["status"] == "error" and recovered["pending"] is None
        online = bridge_state(bridges[0], lambda v: v["connected"])
        assert len(online["drafts"]) == 1
        draft_id = online["drafts"][0]["id"]
        for _ in range(2):
            bridges[0].stdin.write(json.dumps({"action": "send_draft", "id": draft_id}) + "\n")
            bridges[0].stdin.flush()
        wait_for(lambda: len([t for t in api()["tasks"] if t["title"] == "Offline draft"]) == 1)
        assert api("/api/events?after=0")["events"]
        print("PASS: auth/CSRF, idempotency, readiness, ACP stream, permission, form validation, parallelism/resource locks/resume slots, continuation, cancel, timeout, maintenance, crash, restart, multi-client cache, offline draft and explicit sync.")
    finally:
        for process in bridges:
            if process.poll() is not None and process.returncode:
                print("Bridge error:", process.stderr.read())
            process.terminate()
            process.wait(timeout=5)
        if core and core.poll() is None:
            core.send_signal(signal.SIGINT)
            try:
                core.wait(timeout=5)
            except subprocess.TimeoutExpired:
                core.kill()
        if (temp / "core.log").exists() and core and core.returncode not in (0, -9, None):
            print((temp / "core.log").read_text())
