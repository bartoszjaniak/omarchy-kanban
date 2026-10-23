#!/usr/bin/env python3
"""Sandbox installer file operations. System managers/Omarchy are simulated.

Runs the real Rust Core for health checks, but never touches real user config,
launchctl/systemd, SSH or a model. This is not a real macOS deployment test.
On Darwin only `install_core` is exercised for its Linux-only refusal; the
widget and Todoist units are still written in the sandbox (production never
reaches them on Darwin because `install_core` exits first).
"""
import argparse
from contextlib import contextmanager
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

CORE_UNIT = "agent-kanban.service"
IMPORT_UNIT = "agent-kanban-todoist-import.service"
TIMER_UNIT = "agent-kanban-todoist-import.timer"
IMPORT_BIN = Path(".local/bin/omarchy-kanban-todoist-import")
IMPORT_SCRIPT = (ROOT / "todoist-autoimport.py").read_bytes()
SOURCE_MANIFEST = json.loads((ROOT / "plugin" / "manifest.json").read_text())


def user_units(home):
    return home / ".config/systemd/user"


@contextmanager
def sandbox(system, loginctl_raises=False):
    """Run the installer against a throwaway HOME with systemctl/launchctl shimmed."""
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
        args = argparse.Namespace(port=port, opencode=str(FIXTURE), no_timer=False)
        backups = home / "backups"
        environment = {
            "HOME": str(home),
            "XDG_DATA_HOME": str(home / "data"),
            "XDG_CONFIG_HOME": str(home / ".config"),
            "KANBAN_FIXTURE": str(fixture),
            "USER": "tester",
        }

        def run(arguments, **kwargs):
            arguments = [str(a) for a in arguments]
            commands.append(arguments)
            if loginctl_raises and arguments[:1] == ["loginctl"]:
                raise subprocess.CalledProcessError(1, arguments)
            # Only the Core restart launches the real binary; restarting the
            # Todoist timer must not spawn a second Core.
            starting = (
                arguments[:3] == ["systemctl", "--user", "restart"] and arguments[3:4] == [CORE_UNIT]
            ) or arguments[:2] == ["launchctl", "bootstrap"]
            if starting:
                if system == "Linux":
                    unit = (user_units(home) / CORE_UNIT).read_text()
                    executable = shlex.split(next(line.removeprefix("ExecStart=") for line in unit.splitlines() if line.startswith("ExecStart=")))
                else:
                    executable = plistlib.loads((home / "Library/LaunchAgents/local.agent-kanban.core.plist").read_bytes())["ProgramArguments"]
                processes.append(subprocess.Popen(executable, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
            stdout = json.dumps([{"id": "bart.kanban"}]) if arguments[-1:] == ["listPlugins"] else ""
            return subprocess.CompletedProcess(arguments, 0, stdout=stdout)

        with patch.dict(os.environ, environment), \
                patch.object(installer.platform, "system", return_value=system), \
                patch.object(installer, "run", side_effect=run), \
                patch.object(installer.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)):
            try:
                yield home, args, backups, commands
            finally:
                for process in processes:
                    process.send_signal(signal.SIGINT)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()


for system in ("Linux", "Darwin"):
    with sandbox(system) as (home, args, backups, commands):
        if system == "Linux":
            core_id = installer.install_core(args, BINARY, backups)
            assert len(core_id) == 36
            assert (installer.DATA / "token").stat().st_mode & 0o777 == 0o600
            assert (installer.DATA / "identity.json").exists()
        else:
            try:
                installer.install_core(args, BINARY, backups)
            except SystemExit as error:
                assert "Linux" in str(error)
            else:
                raise AssertionError("install_core must refuse Darwin (systemd only)")
            assert not (user_units(home) / CORE_UNIT).exists()

        old_plugin = home / ".config/omarchy/plugins/bart.kanban"
        old_plugin.mkdir(parents=True)
        (old_plugin / "Widget.qml").write_text("USER POC MUST BE BACKED UP")
        (home / ".config/omarchy/shell.json").write_text('{"user":"config"}')
        installer.install_widget(BINARY, backups)
        assert (backups / "bart.kanban/Widget.qml").read_text() == "USER POC MUST BE BACKED UP"
        assert (backups / "shell.json").read_text() == '{"user":"config"}'
        assert not any(command[:3] == ["omarchy", "bar", "move"] for command in commands)
        # Compare against the source manifest instead of a hardcoded version.
        assert json.loads((old_plugin / "manifest.json").read_text())["version"] == SOURCE_MANIFEST["version"]

        mark = len(commands)
        installer.install_todoist_autoimport(args, backups)
        assert commands[mark:] == [
            ["systemctl", "--user", "daemon-reload"],
            ["systemctl", "--user", "enable", "--now", TIMER_UNIT],
            ["systemctl", "--user", "restart", TIMER_UNIT],
            ["loginctl", "enable-linger", "tester"],
        ]

        import_bin = home / IMPORT_BIN
        assert import_bin.exists()
        assert import_bin.stat().st_mode & 0o777 == 0o755
        assert import_bin.read_bytes() == IMPORT_SCRIPT

        service = (user_units(home) / IMPORT_UNIT).read_text()
        for needle in (
            "Type=oneshot",
            "ExecStart=%h/.local/bin/omarchy-kanban-todoist-import",
            "ConditionPathExists",
            'Environment="PATH=',
            "Environment=OPENCODE_BIN=",
            "Environment=KANBAN_IMPORT_MODEL=",
            "RuntimeMaxSec=15min",
            "SyslogIdentifier=kanban-todoist",
            "StandardOutput=journal",
        ):
            assert needle in service, f"service missing {needle!r}"
        assert "[Install]" not in service

        timer = (user_units(home) / TIMER_UNIT).read_text()
        for needle in (
            "OnCalendar=*-*-* 00/2:05:00",
            "Persistent=true",
            "AccuracySec=1min",
            "RandomizedDelaySec=5min",
            f"Unit={IMPORT_UNIT}",
            "WantedBy=timers.target",
        ):
            assert needle in timer, f"timer missing {needle!r}"

        print(f"PASS: {system} installer layout, private token, real health check, POC/config backup, preserved bar placement, Todoist auto-import units (service manager simulated).")

# --no-timer: an existing timer is disabled, never rewritten.
with sandbox("Linux") as (home, args, backups, commands):
    args.no_timer = True
    units = user_units(home)
    units.mkdir(parents=True)
    (units / TIMER_UNIT).write_text("[Timer]\n")
    installer.install_todoist_autoimport(args, backups)
    assert commands == [["systemctl", "--user", "disable", "--now", TIMER_UNIT]]
    assert not (units / IMPORT_UNIT).exists()
    assert (home / IMPORT_BIN).exists()

# --no-timer on a clean host: nothing to disable, no units written.
with sandbox("Linux") as (home, args, backups, commands):
    args.no_timer = True
    installer.install_todoist_autoimport(args, backups)
    assert commands == []
    assert not (user_units(home) / TIMER_UNIT).exists()
    assert (home / IMPORT_BIN).exists()

# Missing opencode must not abort; the unit relies on PATH.
with sandbox("Linux") as (home, args, backups, commands):
    with patch.object(installer.shutil, "which", return_value=None):
        installer.install_todoist_autoimport(args, backups)
    assert "Environment=OPENCODE_BIN=opencode" in (user_units(home) / IMPORT_UNIT).read_text()

# Failing loginctl (no polkit) is a warning, not an install error.
with sandbox("Linux", loginctl_raises=True) as (home, args, backups, commands):
    installer.install_todoist_autoimport(args, backups)
    assert commands[-1] == ["loginctl", "enable-linger", "tester"]
    assert (user_units(home) / TIMER_UNIT).exists()
