#!/usr/bin/env python3
"""Sandbox installer file operations. System managers/Omarchy are simulated.

Runs the real Rust Core for health checks, but never touches real user config,
launchctl/systemd, SSH or a model. This is not a real macOS deployment test.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shlex
import signal
import socket
import subprocess
import tempfile
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("KANBAN_BINARY", ROOT / "target/debug/omarchy-kanban"))
FIXTURE = ROOT / "tests/fake_api.py"
FIXTURE.chmod(0o755)
spec = importlib.util.spec_from_file_location("installer", ROOT / "install.py")
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)

for system in ("Linux", "Darwin"):
    with tempfile.TemporaryDirectory(prefix="kanban-install-", dir="/tmp/opencode") as temporary:
        home = Path(temporary)
        fixture = home / "fixture.json"
        fixture.write_text(json.dumps({"sessions": [], "active": []}))
        installer.HOME = home
        installer.DATA = home / "data/agent-kanban"
        installer.CONFIG = home / ".config/agent-kanban"
        installer.BIN = home / ".local/bin/omarchy-kanban"
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
        listener.close()
        commands = []
        processes = []

        def run(arguments, **kwargs):
            arguments = [str(a) for a in arguments]
            commands.append(arguments)
            starting = arguments[:3] == ["systemctl", "--user", "restart"] or arguments[:2] == ["launchctl", "bootstrap"]
            if starting:
                if system == "Linux":
                    unit = (home / ".config/systemd/user/agent-kanban.service").read_text()
                    executable = shlex.split(next(line.removeprefix("ExecStart=") for line in unit.splitlines() if line.startswith("ExecStart=")))
                else:
                    executable = plistlib.loads((home / "Library/LaunchAgents/local.agent-kanban.core.plist").read_bytes())["ProgramArguments"]
                processes.append(subprocess.Popen(executable, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
            stdout = json.dumps([{"id": "bart.kanban"}]) if arguments[-1:] == ["listPlugins"] else ""
            return subprocess.CompletedProcess(arguments, 0, stdout=stdout)

        args = argparse.Namespace(port=port, opencode=str(FIXTURE))
        backups = home / "backups"
        try:
            with patch.dict(os.environ, {"HOME": str(home), "XDG_DATA_HOME": str(home / "data"), "XDG_CONFIG_HOME": str(home / ".config"), "KANBAN_FIXTURE": str(fixture)}), patch.object(installer.platform, "system", return_value=system), patch.object(installer, "run", side_effect=run), patch.object(installer.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)):
                # Popen uses the underlying process API and is unaffected by the run shim.
                core_id = installer.install_core(args, BINARY, backups)
                assert len(core_id) == 36
                assert (installer.DATA / "token").stat().st_mode & 0o777 == 0o600
                assert (installer.DATA / "identity.json").exists()
                old_plugin = home / ".config/omarchy/plugins/bart.kanban"
                old_plugin.mkdir(parents=True)
                (old_plugin / "Widget.qml").write_text("USER POC MUST BE BACKED UP")
                (home / ".config/omarchy/shell.json").write_text('{"user":"config"}')
                installer.install_widget(BINARY, backups)
                assert (backups / "bart.kanban/Widget.qml").read_text() == "USER POC MUST BE BACKED UP"
                assert (backups / "shell.json").read_text() == '{"user":"config"}'
                assert not any(command[:3] == ["omarchy", "bar", "move"] for command in commands)
                assert json.loads((old_plugin / "manifest.json").read_text())["version"] == "0.3.0"
                print(f"PASS: {system} installer layout, private token, real health check, POC/config backup, preserved bar placement (service manager simulated).")
        finally:
            for process in processes:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
