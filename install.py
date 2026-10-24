#!/usr/bin/env python3
"""Install the personal Kanban board (TODO / In Progress / DONE + archive).

Builds/installs the local Core service and the Omarchy bar widget. Local only:
no SSH, no remote Core. The AI drives the same board through the CLI.
"""
import argparse
from datetime import datetime
import json
import os
import platform
import shutil
import sqlite3
import subprocess
import sys
import time
import urllib.request
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parent
HOME = Path.home()
DATA = Path(os.environ.get("XDG_DATA_HOME", HOME / ("Library/Application Support" if platform.system() == "Darwin" else ".local/share"))) / "agent-kanban"
BIN = HOME / ".local/bin/omarchy-kanban"


def private_write(path, content):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temp = path.with_name(path.name + "." + str(uuid.uuid4()))
    with os.fdopen(os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as handle:
        handle.write(content if isinstance(content, bytes) else content.encode())
    temp.replace(path)


def run(args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def quote(value):
    """systemd: quoted argument; %% escapes specifier expansion."""
    return json.dumps(str(value).replace("%", "%%"))


def unit_env(name, value):
    return "Environment=" + quote(f"{name}={value}")


def backup(path, folder):
    if path.exists():
        folder.mkdir(parents=True, exist_ok=True)
        if path.is_dir():
            shutil.copytree(path, folder / path.name)
        else:
            shutil.copy2(path, folder / path.name)


def build(args):
    if args.binary:
        return Path(args.binary).resolve()
    if (ROOT / "bin/omarchy-kanban").exists():
        return ROOT / "bin/omarchy-kanban"
    cargo = shutil.which("cargo") or str(HOME / ".cargo/bin/cargo")
    run([cargo, "build", "--release", "--locked"], cwd=ROOT)
    return ROOT / "target/release/omarchy-kanban"


def install_core(args, binary, backups):
    DATA.mkdir(parents=True, exist_ok=True, mode=0o700)
    database = DATA / "board.sqlite3"
    if database.exists():
        backups.mkdir(parents=True, exist_ok=True)
        with sqlite3.connect(database) as source, sqlite3.connect(backups / database.name) as target:
            source.backup(target)
    backup(BIN, backups)
    BIN.parent.mkdir(parents=True, exist_ok=True)
    temp = BIN.with_name("omarchy-kanban.new")
    shutil.copy2(binary, temp)
    temp.chmod(0o755)
    temp.replace(BIN)
    path = os.environ.get("PATH", "")
    if platform.system() == "Darwin":
        raise SystemExit("Instalator obsługuje na razie tylko Linux (systemd).")
    service = HOME / ".config/systemd/user/agent-kanban.service"
    backup(service, backups)
    private_write(service, "[Unit]\nDescription=Personal Kanban board (TODO / In Progress / DONE)\nAfter=network.target\n\n[Service]\nType=simple\nExecStart=" + " ".join(quote(v) for v in [BIN, "serve", "--port", args.port]) + "\nRestart=on-failure\nRestartSec=5\nEnvironment=" + quote("PATH=" + path) + "\nUMask=0077\n\n[Install]\nWantedBy=default.target\n")
    run(["systemctl", "--user", "daemon-reload"])
    run(["systemctl", "--user", "enable", "agent-kanban.service"])
    run(["systemctl", "--user", "restart", "agent-kanban.service"])
    for _ in range(40):
        try:
            request = urllib.request.Request(f"http://127.0.0.1:{args.port}/api/state", headers={"Authorization": "Bearer " + (DATA / "token").read_text().strip()})
            with urllib.request.urlopen(request, timeout=1) as response:
                return json.load(response)["core_id"]
        except (OSError, ValueError):
            time.sleep(0.25)
    raise SystemExit("Usługa nie przeszła health-check. Sprawdź journalctl --user -u agent-kanban. Backup zachowano.")


def install_widget(binary, backups):
    config = Path(os.environ.get("XDG_CONFIG_HOME", HOME / ".config")) / "omarchy"
    destination = config / "plugins/bart.kanban"
    backup(destination, backups)
    backup(config / "shell.json", backups)
    destination.mkdir(parents=True, exist_ok=True)
    temporary = destination / "kanban-backend.new"
    shutil.copy2(binary, temporary)
    temporary.chmod(0o755)
    temporary.replace(destination / "kanban-backend")
    for name in ("Widget.qml", "manifest.json"):
        shutil.copy2(ROOT / "plugin" / name, destination / name)
    run(["omarchy-shell", "shell", "rescanPlugins"])
    for _ in range(50):
        plugins = json.loads(run(["omarchy-shell", "shell", "listPlugins"], capture_output=True, text=True).stdout)
        if any(p["id"] == "bart.kanban" for p in plugins):
            break
        time.sleep(0.1)
    run(["omarchy", "plugin", "enable", "bart.kanban"])
    if not any(backups.glob("bart.kanban")):
        run(["omarchy", "bar", "move", "bart.kanban", "--after", "omarchy.weather"])


def install_todoist_autoimport(args, backups):
    """Install the Todoist auto-import script and (unless --no-timer) its timer units."""
    target = HOME / ".local/bin/omarchy-kanban-todoist-import"
    backup(target, backups)
    private_write(target, (ROOT / "todoist-autoimport.py").read_bytes())
    os.chmod(target, 0o755)
    service = HOME / ".config/systemd/user/agent-kanban-todoist-import.service"
    timer = HOME / ".config/systemd/user/agent-kanban-todoist-import.timer"
    if args.no_timer:
        if timer.exists():
            run(["systemctl", "--user", "disable", "--now", "agent-kanban-todoist-import.timer"])
        return
    backup(service, backups)
    backup(timer, backups)
    private_write(service, "\n".join([
        "[Unit]",
        "Description=Kanban: auto-import z Todoist (AI)",
        "After=network.target",
        "ConditionPathExists=%h/.local/bin/omarchy-kanban-todoist-import",
        "",
        "[Service]",
        "Type=oneshot",
        "ExecStart=%h/.local/bin/omarchy-kanban-todoist-import",
        "Environment=" + quote("PATH=" + os.environ.get("PATH", "")),
        unit_env("OPENCODE_BIN", shutil.which("opencode") or "opencode"),
        unit_env("KANBAN_IMPORT_MODEL", os.environ.get("KANBAN_IMPORT_MODEL", "opencode/glm-5.3-flash-free")),
        "StandardOutput=journal",
        "StandardError=journal",
        "SyslogIdentifier=kanban-todoist",
        "TimeoutStartSec=15min",
        "UMask=0077",
        "",
    ]))
    private_write(timer, "\n".join([
        "[Unit]",
        "Description=Kanban: auto-import z Todoist co godzinę (07:00\u201323:00)",
        "",
        "[Timer]",
        "OnCalendar=*-*-* 07..23:00:00",
        "Persistent=true",
        "AccuracySec=1min",
        "RandomizedDelaySec=5min",
        "Unit=agent-kanban-todoist-import.service",
        "",
        "[Install]",
        "WantedBy=timers.target",
        "",
    ]))
    run(["systemctl", "--user", "daemon-reload"])
    run(["systemctl", "--user", "enable", "--now", "agent-kanban-todoist-import.timer"])
    run(["systemctl", "--user", "restart", "agent-kanban-todoist-import.timer"])
    user = os.environ.get("USER") or os.environ.get("LOGNAME")
    if user:
        try:
            run(["loginctl", "enable-linger", user])
        except (OSError, subprocess.CalledProcessError) as error:
            print(f"Ostrzeżenie: loginctl enable-linger nie powiodło się: {error}", file=sys.stderr)
    else:
        print("Ostrzeżenie: nie znam nazwy użytkownika — pominięto loginctl enable-linger", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", help="Prebuilt binary for this host instead of a source build")
    parser.add_argument("--port", type=int, default=4765)
    parser.add_argument("--no-widget", action="store_true")
    parser.add_argument("--no-timer", action="store_true", help="Nie instaluj timera auto-importu z Todoist; wyłącz istniejący, jeśli jest")
    parser.add_argument("--yes", action="store_true", help="Approve the printed install plan")
    args = parser.parse_args()
    if not 1 <= args.port <= 65535:
        parser.error("port: 1–65535")
    args.backups = DATA / "backups" / datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    if not args.yes:
        raise SystemExit("Plan: build/install the board Core, back up board.sqlite3, install/start the local service, update the Omarchy widget. Use --yes to approve.")
    binary = build(args)
    install_core(args, binary, args.backups)
    if not args.no_widget:
        install_widget(binary, args.backups)
    install_todoist_autoimport(args, args.backups)
    print(f"Installed. Backups: {args.backups}")
    print("Widget → licznik. CLI dla AI: omarchy-kanban task add \"…\" / list / move / edit / rm")
    if not args.no_timer:
        print("Auto-import z Todoist: timer ~/.config/systemd/user/agent-kanban-todoist-import.timer; podgląd logu: journalctl --user -u agent-kanban-todoist-import -n 50")


if __name__ == "__main__":
    main()
