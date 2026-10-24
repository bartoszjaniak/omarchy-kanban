#!/usr/bin/env python3
"""Auto-import zadań z Todoist przy pomocy modelu (timer systemd).

Orkiestruje pełny cykl poza usługą Core: sprawdza status integracji, pobiera
kandydatów przez CLI `omarchy-kanban`, pyta model przez `opencode run`, waliduje
odpowiedź i woła `omarchy-kanban todoist import`. Skrypt używa wyłącznie
publicznego CLI — nie widzi tokenu Todoist ani Core. Awaria skryptu nie dotyka
pętli synchronizacji. Każda linia logu idzie na stderr z prefiksem
`todoist-import:`.

Użycie:
    omarchy-kanban-todoist-import            # pełny cykl (timer)
    omarchy-kanban-todoist-import --dry-run  # pokaż decyzję modelu, nic nie importuj
"""
import argparse
import datetime
import fcntl
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

DEFAULT_MODEL = "opencode/glm-5.3-flash-free"
DEFAULT_LIMIT = 10
DEFAULT_MAX_CANDIDATES = 100
DEFAULT_KANBAN = "omarchy-kanban"
DEFAULT_OPENCODE = "opencode"

# Okno aktywności (lokalny czas): 07:00–23:59. Chroni przed nocnym cyklem
# nadrabiającym `Persistent=true` po starcie/wznowieniu (§8.1 pkt 1).
ACTIVE_HOUR_FIRST = 7
ACTIVE_HOUR_LAST = 23

LOCK_NAME = "kanban-todoist-import.lock"
DESCRIPTION_LIMIT = 300
REASON_LIMIT = 120

STATUS_TIMEOUT = 30
CANDIDATES_TIMEOUT = 30
MODEL_TIMEOUT = 180
IMPORT_TIMEOUT = 120

# Kontrakt promptu (§7.2): zmiana treści = zmiana zachowania. Podstawiamy
# wyłącznie `<<<ZADANIA>>>`.
PROMPT = """Jesteś filtrem zadań. Otrzymasz listę zadań z Todoist (JSON).
Zdecyduj, które z nich da się wykonać przy komputerze, siedząc przed nim.

Komputerowo wykonalne: pisanie, kodowanie, wysyłanie maili i ofert, zamawianie
rzeczy online, wypełnianie formularzy, research, rozmowy, płatności, rezerwacje.
Niewykonalne: prace fizyczne w domu i warsztacie (sprzątanie, mycie, malowanie,
odkamienianie, naprawy, montaż), wizyty, zakupy w sklepie stacjonarnym.
Brak opisu nie jest błędem — oceń po treści zadania.

Zasady odpowiedzi:
- Zwróć WYŁĄCZNIE obiekt JSON, bez prozy, komentarzy i bloków markdown.
- Format: {"import":[{"id":"<id>","reason":"<jedno zdanie, max 120 znaków>"}]}
- "id" kopiuj dosłownie z listy; nigdy nie wymyślaj ani nie modyfikuj id.
- Zadanie niewykonalne po prostu pomiń (nie wypisuj go).
- Gdy nic nie jest wykonalne: {"import":[]}

Zadania:
<<<ZADANIA>>>"""


class CycleError(Exception):
    """Błąd przerywający cykl importu (log + exit 1)."""


def log(message):
    """Jedna linia logu na stderr z prefiksem journald-niezależnym."""
    print("todoist-import: " + message, file=sys.stderr)


def within_active_window(now=None):
    """Czy lokalna godzina mieści się w oknie 07:00–23:59 (włącznie)."""
    moment = now or datetime.datetime.now()
    return ACTIVE_HOUR_FIRST <= moment.hour <= ACTIVE_HOUR_LAST


def int_env(name, default):
    """Wczytuje dodatnią liczbę z env; zła wartość → domyślna."""
    raw = os.environ.get(name)
    if raw is None:
        return default
    try:
        value = int(raw)
    except ValueError:
        log(f"ignoruję {name}={raw!r} (nie liczba)")
        return default
    return value if value > 0 else default


def run_capture(args, timeout):
    """Uruchamia polecenie, przechwytuje wyjście; błędy środowiska → CycleError."""
    try:
        return subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    except FileNotFoundError:
        raise CycleError(f"brak polecenia {args[0]}") from None
    except subprocess.TimeoutExpired:
        raise CycleError(f"timeout {timeout}s: {args[0]}") from None
    except OSError as error:
        raise CycleError(f"nie uruchomiono {args[0]}: {error}") from None


def failure(result, what):
    """CycleError z ostatnią linią stderr, żeby journald miał przyczynę."""
    detail = (result.stderr or "").strip().splitlines()
    suffix = f": {detail[-1][:200]}" if detail else ""
    return CycleError(f"{what} zakończył się kodem {result.returncode}{suffix}")


def parse_json(text, what):
    """Parsuje JSON z wyjścia CLI; brak poprawnego JSON-a → CycleError."""
    try:
        return json.loads(text)
    except ValueError as error:
        raise CycleError(f"nieprawidłowy JSON ({what}): {error}") from None


def lock_path():
    """Ścieżka locka: XDG_RUNTIME_DIR → /run/user/<uid> → katalog tymczasowy."""
    directories = []
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime:
        directories.append(Path(runtime))
    directories.append(Path(f"/run/user/{os.getuid()}"))
    directories.append(Path(tempfile.gettempdir()))
    for directory in directories:
        if directory.is_dir():
            return directory / LOCK_NAME
    return Path(tempfile.gettempdir()) / LOCK_NAME


def acquire_lock():
    """Nieblokujący flock; zwraca deskryptor albo None, gdy cykl już trwa."""
    path = lock_path()
    try:
        handle = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    except OSError as error:
        raise CycleError(f"nie otworzono locka {path}: {error}") from None
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        os.close(handle)
        return None
    return handle


def read_status(binary):
    """`todoist status --json`; kod ≠0 → CycleError."""
    result = run_capture([binary, "todoist", "status", "--json"], STATUS_TIMEOUT)
    if result.returncode != 0:
        raise failure(result, "status")
    return parse_json(result.stdout, "status")


def read_candidates(binary):
    """`todoist candidates --json`; zwraca listę zadań, kod ≠0 → CycleError."""
    result = run_capture([binary, "todoist", "candidates", "--json"], CANDIDATES_TIMEOUT)
    if result.returncode != 0:
        raise failure(result, "candidates")
    payload = parse_json(result.stdout, "candidates")
    tasks = payload.get("tasks") if isinstance(payload, dict) else None
    if not isinstance(tasks, list):
        raise CycleError("candidates bez listy tasks")
    return tasks


def prompt_tasks(tasks):
    """Zadania do promptu: id, content, description (≤300), due."""
    return [
        {
            "id": task.get("id", ""),
            "content": task.get("content", ""),
            "description": (task.get("description") or "")[:DESCRIPTION_LIMIT],
            "due": task.get("due"),
        }
        for task in tasks
    ]


def build_prompt(tasks):
    """Stały tekst z podstawionym JSON-em zadań."""
    payload = json.dumps(prompt_tasks(tasks), ensure_ascii=False, indent=2)
    return PROMPT.replace("<<<ZADANIA>>>", payload)


def ask_model(opencode, model, prompt):
    """`opencode run --standalone --model <model> <prompt>`; kod ≠0 → CycleError."""
    args = [opencode, "run", "--standalone", "--model", model, prompt]
    result = run_capture(args, MODEL_TIMEOUT)
    if result.returncode != 0:
        raise failure(result, "opencode")
    return result.stdout or ""


def parse_decision(text, candidate_ids, limit):
    """Waliduje odpowiedź modelu wg §7.3.

    Zwraca (przyjete, odrzucone, czy_pusto): `przyjete` to lista (id, reason),
    `odrzucone` to lista (id, powod). Strukturalne błędy (brak JSON-a, brak
    listy `import`) → CycleError. Sama lista `import: []` → czy_pusto=True.
    """
    start = text.find("{")
    if start < 0:
        raise CycleError("odpowiedź modelu bez JSON-a")
    try:
        payload, _ = json.JSONDecoder().raw_decode(text[start:])
    except ValueError as error:
        raise CycleError(f"nieparsowalny JSON w odpowiedzi: {error}") from None
    if not isinstance(payload, dict) or not isinstance(payload.get("import"), list):
        raise CycleError("odpowiedź modelu bez listy import")

    accepted = []
    rejected = []
    seen = set()
    for item in payload["import"]:
        if not isinstance(item, dict):
            rejected.append((None, "zly_ksztalt"))
            continue
        identifier = item.get("id")
        if not isinstance(identifier, str):
            rejected.append((None, "zly_ksztalt"))
            continue
        if identifier not in candidate_ids:
            rejected.append((identifier, "spoza_kandydatow"))
            continue
        if identifier in seen:
            rejected.append((identifier, "duplikat"))
            continue
        seen.add(identifier)
        reason = item.get("reason")
        reason = reason[:REASON_LIMIT] if isinstance(reason, str) else ""
        accepted.append((identifier, reason))

    if not payload["import"]:
        return [], [], True

    if len(accepted) > limit:
        for identifier, _ in accepted[limit:]:
            rejected.append((identifier, "ponad_limit"))
        accepted = accepted[:limit]
    return accepted, rejected, False


def run_import(binary, accepted):
    """`todoist import <id>…` — jedno wywołanie, bez `reason` w argv."""
    ids = [identifier for identifier, _ in accepted]
    result = run_capture([binary, "todoist", "import", *ids], IMPORT_TIMEOUT)
    output = (result.stdout or "").strip()
    if output:
        for line in output.splitlines():
            log(line)
    if result.returncode != 0:
        raise failure(result, "import")


def summary(candidates, prompt_count, imported, rejected):
    """Jedna linia podsumowania cyklu (§8.1 pkt 8)."""
    return (
        f"cykl kandydaci={candidates} do_promptu={prompt_count} "
        f"import={imported} odrzucone={rejected}"
    )


def cycle(binary, opencode, model, limit, max_candidates, dry_run):
    """Pełny cykl auto-importu; zwraca kod wyjścia."""
    handle = acquire_lock()
    if handle is None:
        log("poprzedni cykl trwa")
        return 0
    try:
        status = read_status(binary)
        if not status.get("enabled"):
            log("integracja Todoist wyłączona")
            return 0
        if status.get("last_error"):
            log(f"ostrzeżenie: ostatnia synchronizacja zgłosiła: {status['last_error']}")

        tasks = read_candidates(binary)
        candidate_count = len(tasks)
        if not tasks:
            log("brak kandydatów")
            log(summary(0, 0, 0, 0))
            return 0

        selected = tasks[:max_candidates]
        prompt_count = len(selected)
        if candidate_count > prompt_count:
            log(f"kandydaci={candidate_count} do_promptu={prompt_count}")

        answer = ask_model(opencode, model, build_prompt(selected))
        candidate_ids = {task.get("id") for task in selected}
        accepted, rejected, empty = parse_decision(answer, candidate_ids, limit)

        for identifier, why in rejected:
            log(f"odrzucone {identifier or '?'}: {why}")
        rejected_count = len(rejected)

        if empty:
            log("model: nic nie jest wykonalne")
            log(summary(candidate_count, prompt_count, 0, rejected_count))
            return 0
        if not accepted:
            raise CycleError("żaden element odpowiedzi nie przeszedł walidacji")

        if dry_run:
            for identifier, reason in accepted:
                log(f"dry-run: {identifier} ({reason or 'bez uzasadnienia'})")
            log(summary(candidate_count, prompt_count, len(accepted), rejected_count))
            return 0

        run_import(binary, accepted)
        log(summary(candidate_count, prompt_count, len(accepted), rejected_count))
        return 0
    finally:
        os.close(handle)


def main():
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--dry-run", action="store_true", help="pokaż decyzję modelu bez importu")
    args = parser.parse_args()

    if not within_active_window():
        log("pominięto: poza lokalnym oknem 07:00-23:59 (nighttime guard)")
        sys.exit(0)

    binary = os.environ.get("KANBAN_BIN", DEFAULT_KANBAN)
    opencode = os.environ.get("OPENCODE_BIN", DEFAULT_OPENCODE)
    model = os.environ.get("KANBAN_IMPORT_MODEL", DEFAULT_MODEL)
    limit = int_env("KANBAN_IMPORT_LIMIT", DEFAULT_LIMIT)
    max_candidates = int_env("KANBAN_IMPORT_MAX_CANDIDATES", DEFAULT_MAX_CANDIDATES)

    try:
        code = cycle(binary, opencode, model, limit, max_candidates, args.dry_run)
    except CycleError as error:
        log(str(error))
        code = 1
    sys.exit(code)


if __name__ == "__main__":
    main()
