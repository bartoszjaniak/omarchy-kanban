#!/usr/bin/env python3
"""Instalacja tokenu Todoist dla lokalnej usługi agent-kanban.

Wczytuje token (getpass przy TTY, pierwsza linia stdin gdy piped), waliduje go
żądaniem do API Todoist, a następnie zapisuje jako `EnvironmentFile` dla usługi
systemd oraz drop-in z `EnvironmentFile`. Token nigdy nie trafia do argv,
historii shella, unit file ani logów.

Użycie:
    python3 todoist-token.py            # pyta o token (getpass lub stdin), waliduje, instaluje
    python3 todoist-token.py --remove   # usuwa integrację i restartuje usługę
"""
import argparse
import getpass
import json
import os
import platform
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

HOME = Path.home()
CONFIG = Path(os.environ.get("XDG_CONFIG_HOME", HOME / ".config"))
DATA = Path(os.environ.get("XDG_DATA_HOME", HOME / ".local/share")) / "agent-kanban"
ENV_FILE = CONFIG / "agent-kanban" / "todoist.env"
DROPIN = CONFIG / "systemd/user/agent-kanban.service.d/10-todoist.conf"
CORE_TOKEN = DATA / "token"
SERVICE = "agent-kanban.service"
DEFAULT_BASE = "https://api.todoist.com/api/v1"
CORE_BASE = "http://127.0.0.1:4765"
DROPIN_BODY = "[Service]\nEnvironmentFile=%h/.config/agent-kanban/todoist.env\n"


def private_write(path, content, dry_run=False):
    """Zapisuje plik 0600 w katalogu 0700, atomowo przez plik tymczasowy."""
    if dry_run:
        print(f"[dry-run] zapisałbym {path} (0600, katalog 0700)")
        return
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temp = path.with_name(path.name + "." + str(uuid.uuid4()))
    with os.fdopen(os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as handle:
        handle.write(content.encode())
    temp.replace(path)


def run(args, dry_run=False):
    """Uruchamia komendę systemową; w trybie dry-run tylko ją wypisuje."""
    printable = " ".join(args)
    if dry_run:
        print(f"[dry-run] {printable}")
        return
    subprocess.run(args, check=True)


def die(message):
    print(message, file=sys.stderr)
    raise SystemExit(1)


def read_token():
    """Wczytuje token z getpass (TTY) albo pierwszej linii stdin (piped)."""
    if sys.stdin.isatty():
        token = getpass.getpass("Todoist API token: ")
    else:
        token = sys.stdin.readline()
    token = token.strip()
    if not token:
        die("Token jest pusty.")
    if any(char.isspace() for char in token):
        die("Token nie może zawierać białych znaków w środku.")
    if not 20 <= len(token) <= 200:
        die("Token musi mieć od 20 do 200 znaków.")
    return token


def validate_token(base_url, token):
    """Sprawdza token żądaniem GET {base}/projects?limit=1. Token nie jest logowany."""
    url = base_url.rstrip("/") + "/projects?limit=1"
    request = urllib.request.Request(url, headers={"Authorization": "Bearer " + token})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            response.read()
    except urllib.error.HTTPError as error:
        if error.code in (401, 403):
            die(f"Token odrzucony przez Todoist (HTTP {error.code}).")
        die(f"Todoist zwrócił błąd HTTP {error.code} podczas walidacji tokenu.")
    except urllib.error.URLError as error:
        die(f"Nie udało się połączyć z Todoist: {error.reason}. Sprawdź sieć i --base-url.")
    except OSError as error:
        die(f"Nie udało się połączyć z Todoist: {error}. Sprawdź sieć i --base-url.")


def health_check(dry_run=False):
    """Czeka do 10 s na /api/state Core z tokenem z katalogu danych."""
    if dry_run:
        print("[dry-run] sprawdziłbym health-check Core na " + CORE_BASE + "/api/state")
        return True
    if not CORE_TOKEN.exists():
        print(f"Brak tokenu Core ({CORE_TOKEN}) — pomijam health-check.", file=sys.stderr)
        return False
    token = CORE_TOKEN.read_text().strip()
    request = urllib.request.Request(CORE_BASE + "/api/state", headers={"Authorization": "Bearer " + token})
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(request, timeout=1) as response:
                json.load(response)
                return True
        except (OSError, ValueError):
            time.sleep(0.25)
    print("Usługa nie przeszła health-check. Sprawdź journalctl --user -u " + SERVICE, file=sys.stderr)
    return False


def print_status():
    """Best-effort `omarchy-kanban todoist status`; brak komendy nie jest błędem."""
    binary = shutil.which("omarchy-kanban") or str(HOME / ".local/bin/omarchy-kanban")
    try:
        result = subprocess.run([binary, "todoist", "status"], capture_output=True, text=True)
    except OSError as error:
        print(f"(nie uruchomiono `omarchy-kanban todoist status`: {error})")
        return
    output = (result.stdout or "").strip() or (result.stderr or "").strip()
    if output:
        print(output)


def install(base_url, dry_run=False):
    token = read_token()
    validate_token(base_url, token)
    private_write(ENV_FILE, f"TODOIST_API_TOKEN={token}\n", dry_run)
    private_write(DROPIN, DROPIN_BODY, dry_run)
    run(["systemctl", "--user", "daemon-reload"], dry_run)
    run(["systemctl", "--user", "restart", SERVICE], dry_run)
    if not health_check(dry_run):
        return
    print("Integracja Todoist aktywna.")
    print(f"Token w env usługi ({SERVICE}), plik {ENV_FILE} (0600).")
    print("Aby usunąć: python3 todoist-token.py --remove")
    print_status()


def remove(dry_run=False):
    removed = []
    for path in (ENV_FILE, DROPIN):
        if path.exists():
            if dry_run:
                print(f"[dry-run] usunąłbym {path}")
            else:
                path.unlink()
            removed.append(path)
    run(["systemctl", "--user", "daemon-reload"], dry_run)
    run(["systemctl", "--user", "restart", SERVICE], dry_run)
    if removed:
        print("Integracja Todoist wyłączona (usunięto " + ", ".join(str(path) for path in removed) + ").")
    else:
        print("Integracja Todoist już wyłączona — nic do usunięcia.")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--remove", action="store_true", help="usuń EnvironmentFile i drop-in, zrestartuj usługę")
    parser.add_argument("--base-url", default=DEFAULT_BASE, help=f"adres API Todoist (domyślnie {DEFAULT_BASE})")
    parser.add_argument("--dry-run", action="store_true", help="pokaż plan bez zapisu plików i bez dotykania systemd")
    args = parser.parse_args()
    if platform.system() != "Linux":
        raise SystemExit("Skrypt obsługuje tylko Linux (systemd).")
    if args.remove:
        remove(args.dry_run)
    else:
        install(args.base_url, args.dry_run)


if __name__ == "__main__":
    main()
