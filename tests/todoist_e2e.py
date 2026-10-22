#!/usr/bin/env python3
"""Black-box E2E test of the Todoist integration: the real Core binary plus a
fake Todoist REST v1 server, isolated in a temp data dir. Standard library only.

Run: python3 tests/todoist_e2e.py
"""
import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("KANBAN_BINARY", ROOT / "target/debug/omarchy-kanban"))
TOKEN_SCRIPT = ROOT / "todoist-token.py"
# Must be >=20 chars: the token length the real client and todoist-token.py demand.
SENTINEL = "sentinel-todoist-token-0123456789"
TASK = {
    "id": "t-1",
    "content": "Kup mleko",
    "description": "2%",
    "due": {"date": "2026-10-22", "string": "dziś"},
    "labels": ["dom"],
    "priority": 2,
    "project_id": "p-1",
}


def wait_for(callback, timeout=15):
    """Poll until `callback` returns a truthy value (no fixed sleeps)."""
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
    raise AssertionError(f"Przekroczono limit czasu: {last}")


def iso(epoch):
    return datetime.fromtimestamp(epoch, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class FakeTodoist:
    """Minimal REST v1 stub: bearer-guarded, records every request."""

    def __init__(self, token):
        self.token = token
        self.lock = threading.Lock()
        self.requests = []  # (method, path, query)
        self.completions = []  # (task_id, epoch)
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def _send(self, code, body=None):
                data = b"" if body is None else json.dumps(body).encode()
                self.send_response(code)
                if data:
                    self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                if data:
                    self.wfile.write(data)

            def _route(self):
                url = urlsplit(self.path)
                with fake.lock:
                    fake.requests.append((self.command, url.path, url.query))
                if self.headers.get("Authorization") != "Bearer " + fake.token:
                    return self._send(401, {"error": "unauthorized"})
                path = url.path
                if self.command == "POST" and path == "/api/v1/tasks/t-1/close":
                    return self._send(204)
                if self.command == "POST" and path == "/api/v1/tasks/t-1/reopen":
                    return self._send(204)
                if self.command == "GET" and path == "/api/v1/projects":
                    return self._send(200, {"results": []})
                if self.command == "GET" and path == "/api/v1/tasks/filter":
                    return self._send(200, {"results": [TASK], "next_cursor": None})
                if self.command == "GET" and path == "/api/v1/tasks/completed/by_completion_date":
                    with fake.lock:
                        items = list(fake.completions)
                    return self._send(
                        200,
                        {
                            "items": [
                                {"id": task_id, "completed_at": iso(epoch)}
                                for task_id, epoch in items
                            ],
                            "next_cursor": None,
                        },
                    )
                if self.command == "GET" and path == "/api/v1/tasks/t-1":
                    return self._send(200, TASK)
                return self._send(404, {"error": "not found"})

            do_GET = _route
            do_POST = _route
            do_DELETE = _route
            do_PUT = _route

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def base_url(self):
        return f"http://127.0.0.1:{self.port}/api/v1"

    def start(self):
        self.thread.start()
        return self

    def stop(self):
        self.server.shutdown()
        self.server.server_close()

    def complete(self, task_id, epoch):
        with self.lock:
            self.completions.append((task_id, epoch))

    def calls(self, method=None, path=None):
        with self.lock:
            return [
                (m, p, q)
                for (m, p, q) in self.requests
                if (method is None or m == method) and (path is None or p == path)
            ]


def start_core(data_dir, core_port, env):
    data_dir.mkdir(parents=True, exist_ok=True)
    return subprocess.Popen(
        [BINARY, "--data-dir", str(data_dir), "serve", "--port", str(core_port)],
        stdout=open(data_dir / "core.out", "wb"),
        stderr=open(data_dir / "core.err", "wb"),
        env=env,
    )


def http(port, method, path, body=None, token=None):
    """Return (status, parsed_json). Never raises on HTTP errors."""
    headers = {"content-type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}", data=data, headers=headers, method=method
    )
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        raw = error.read()
        try:
            parsed = json.loads(raw)
        except ValueError:
            parsed = {"raw": raw.decode(errors="replace")}
        return error.code, parsed


def identity(data_dir):
    """Wait for Core to bind, then return (port, token)."""
    wait_for(lambda: (data_dir / "identity.json").exists() and (data_dir / "token").exists())
    written = json.loads((data_dir / "identity.json").read_text())
    return written["port"], (data_dir / "token").read_text().strip()


def main():
    temps = tempfile.TemporaryDirectory(
        prefix="kanban-todoist-", dir="/tmp/opencode" if Path("/tmp/opencode").exists() else None
    )
    root = Path(temps.name)
    fake = FakeTodoist(SENTINEL).start()
    fake_off = FakeTodoist(SENTINEL).start()
    cores = []
    try:
        data_dir = root / "core"
        env = {
            **os.environ,
            "TODOIST_API_TOKEN": SENTINEL,
            "TODOIST_API_URL": fake.base_url,
        }
        core = start_core(data_dir, 0, env)
        cores.append(core)
        port, token = identity(data_dir)

        def sync():
            def call():
                status, body = http(port, "POST", "/api/todoist/sync", token=token)
                assert status == 200, f"sync zwrócił HTTP {status}: {body}"
                return None if body.get("busy") else body
            return wait_for(call)

        def state():
            status, body = http(port, "GET", "/api/state", token=token)
            assert status == 200, f"/api/state zwrócił HTTP {status}"
            return body

        def card(todoist_id="t-1"):
            return next(t for t in state()["tasks"] if t.get("todoist_id") == todoist_id)

        # 1. Status and candidates on a fresh, linked store.
        status, body = http(port, "GET", "/api/todoist/status", token=token)
        assert status == 200 and body["enabled"] is True, f"status powinien być enabled: {body}"
        assert body["linked"] == 0, f"świeży store ma 0 linków, jest {body['linked']}"
        status, body = http(port, "GET", "/api/todoist/candidates", token=token)
        assert status == 200, f"candidates zwróciły HTTP {status}"
        ids = [t["id"] for t in body["tasks"]]
        assert ids == ["t-1"], f"kandydaci powinni zawierać t-1, jest {ids}"

        # 2. Import once, then again: note with the due date, no duplicate.
        status, body = http(port, "POST", "/api/todoist/import", {"ids": ["t-1"], "status": "todo"}, token)
        assert status == 200, f"import zwrócił HTTP {status}: {body}"
        assert len(body["imported"]) == 1 and body["skipped"] == [], f"pierwszy import: {body}"
        assert "Termin:" in card()["notes"], f"karta bez terminu w notatce: {card()['notes']!r}"

        status, body = http(port, "POST", "/api/todoist/import", {"ids": ["t-1"], "status": "todo"}, token)
        assert status == 200, f"drugi import zwrócił HTTP {status}"
        assert len(body["imported"]) == 0 and len(body["skipped"]) == 1, f"drugi import: {body}"
        assert body["skipped"][0]["reason"] == "already_linked", f"powód pominięcia: {body['skipped']}"
        assert len([t for t in state()["tasks"] if t.get("todoist_id") == "t-1"]) == 1, "import utworzył duplikat"

        # 3. Card -> DONE pushes exactly one close.
        identifier = card()["id"]
        status, _ = http(port, "PATCH", f"/api/tasks/{identifier}", {"status": "done"}, token)
        assert status == 200, f"PATCH done zwrócił HTTP {status}"
        sync()
        closes = fake.calls("POST", "/api/v1/tasks/t-1/close")
        assert len(closes) == 1, f"oczekiwano jednego close, jest {len(closes)}: {closes}"

        # 4. DONE -> TODO pushes exactly one reopen. Wait for a later wall-clock
        # second so the next cycle sees the card as untouched (pull precondition).
        before = int(time.time())
        status, _ = http(port, "PATCH", f"/api/tasks/{identifier}", {"status": "todo"}, token)
        assert status == 200, f"PATCH todo zwrócił HTTP {status}"
        wait_for(lambda: int(time.time()) > before + 1, timeout=5)
        sync()
        reopens = fake.calls("POST", "/api/v1/tasks/t-1/reopen")
        assert len(reopens) == 1, f"oczekiwano jednego reopen, jest {len(reopens)}: {reopens}"

        # 5. A completion in Todoist pulls the card to DONE; a later cycle is a no-op.
        fake.complete("t-1", int(time.time()))
        report = sync()
        assert report["pulled"] == 1, f"oczekiwano 1 przeciągniętego, jest {report}"
        pulled = card()
        assert pulled["status"] == "done", f"karta powinna być done, jest {pulled['status']}"
        assert pulled["done_at"] is not None, "done_at nie zostało ustawione"
        sync()
        assert len(fake.calls("POST", "/api/v1/tasks/t-1/close")) == 1, "kolejny sync wysłał zbędny close"

        # 6. Deleting a linked card is local only: no outbound write.
        writes_before = len(fake.calls("POST")) + len(fake.calls("DELETE"))
        status, _ = http(port, "DELETE", f"/api/tasks/{identifier}", token=token)
        assert status == 200, f"DELETE zwrócił HTTP {status}"
        assert len(fake.calls("POST")) + len(fake.calls("DELETE")) == writes_before, (
            "usunięcie karty wysłało żądanie do Todoist"
        )

        # 7. The token never leaks: cmdline, Core logs, or `todoist status`.
        core_cmdline = Path(f"/proc/{core.pid}/cmdline").read_bytes().decode(errors="replace")
        assert SENTINEL not in core_cmdline, "token wyciekł do cmdline Core"
        logs = (data_dir / "core.out").read_bytes() + (data_dir / "core.err").read_bytes()
        assert SENTINEL.encode() not in logs, "token wyciekł do logów Core"
        cli = subprocess.run(
            [BINARY, "--data-dir", str(data_dir), "todoist", "status"],
            capture_output=True,
        )
        assert cli.returncode == 0, f"`todoist status` zwrócił kod {cli.returncode}: {cli.stderr!r}"
        assert SENTINEL not in (cli.stdout + cli.stderr).decode(errors="replace"), (
            "token wyciekł do `todoist status`"
        )

        # 8. A second Core without TODOIST_API_TOKEN: off, 400, zero requests.
        off_dir = root / "core-off"
        off_env = {k: v for k, v in env.items() if k not in ("TODOIST_API_TOKEN", "TODOIST_API_URL")}
        off_env["TODOIST_API_URL"] = fake_off.base_url
        off_core = start_core(off_dir, 0, off_env)
        cores.append(off_core)
        off_port, off_token = identity(off_dir)
        status, body = http(off_port, "GET", "/api/todoist/status", token=off_token)
        assert status == 200 and body["enabled"] is False, f"bez tokenu enabled ma być false: {body}"
        status, body = http(off_port, "GET", "/api/todoist/candidates", token=off_token)
        assert status == 400, f"candidates bez tokenu: oczekiwano 400, jest {status}"
        assert "wyłącz" in body.get("error", ""), f"komunikat o wyłączonej integracji: {body}"
        assert fake_off.requests == [], f"wyłączony Core wysłał żądania: {fake_off.requests}"

        # 9. todoist-token.py --dry-run: token only via stdin, no files written.
        config = root / "config"
        args = [sys.executable, str(TOKEN_SCRIPT), "--dry-run", "--base-url", fake.base_url]
        assert SENTINEL not in args, "token trafił do argv skryptu"
        token_env = {
            **os.environ,
            "HOME": str(root / "home"),
            "XDG_CONFIG_HOME": str(config),
            "XDG_DATA_HOME": str(root / "data"),
        }
        script = subprocess.run(args, input=SENTINEL + "\n", capture_output=True, text=True, env=token_env)
        assert script.returncode == 0, f"todoist-token.py --dry-run zwrócił {script.returncode}: {script.stderr!r}"
        assert not (config / "agent-kanban/todoist.env").exists(), "dry-run zapisał todoist.env"
        assert not (config / "systemd/user/agent-kanban.service.d/10-todoist.conf").exists(), (
            "dry-run zapisał drop-in systemd"
        )

        print("OK")
    finally:
        for core in cores:
            if core.poll() is None:
                core.send_signal(signal.SIGINT)
                try:
                    core.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    core.kill()
        fake.stop()
        fake_off.stop()
        temps.cleanup()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:  # noqa: BLE001 - report any failure as a clean exit 1
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
