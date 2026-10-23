#!/usr/bin/env python3
"""Process test of `todoist-autoimport.py` (§10, 18 cases) with stub CLIs.

`KANBAN_BIN` and `OPENCODE_BIN` are replaced by throwaway stubs in a temp dir;
no real model and no live service is touched. Each case gets its own temp dir
(lock + logs isolated) and `XDG_RUNTIME_DIR` is pointed there so `flock` never
touches `/run/user`. Standard library only.

Run: python3 tests/todoist_autoimport.py
"""
import fcntl
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "todoist-autoimport.py"

# Stub `omarchy-kanban`: logs every argv, then answers a command. Behaviour is
# driven by env (STUB_STATUS/STUB_CANDIDATES/STUB_IMPORT_*) so each case tweaks
# only what it needs.
STUB_KANBAN = """#!{python}
import json, os, sys
args = sys.argv[1:]
with open(os.environ["KANBAN_LOG"], "a") as fh:
    fh.write(json.dumps(args) + "\\n")

def emit(payload_name, exit_name):
    payload = os.environ.get(payload_name)
    if payload is not None:
        sys.stdout.write(payload)
    sys.exit(int(os.environ.get(exit_name, "0")))

if args[:2] == ["todoist", "status"]:
    emit("STUB_STATUS", "STUB_STATUS_EXIT")
if args[:2] == ["todoist", "candidates"]:
    emit("STUB_CANDIDATES", "STUB_CANDIDATES_EXIT")
if args[:2] == ["todoist", "import"]:
    out = os.environ.get("STUB_IMPORT_OUT", "")
    if out:
        sys.stdout.write(out)
    sys.exit(int(os.environ.get("STUB_IMPORT_EXIT", "0")))
sys.exit(2)
""".format(python=sys.executable)

# Stub `opencode`: logs argv, stores the prompt, prints a prepared answer.
STUB_OPENCODE = """#!{python}
import json, os, sys
args = sys.argv[1:]
with open(os.environ["OPENCODE_LOG"], "a") as fh:
    fh.write(json.dumps(args) + "\\n")
with open(os.environ["PROMPT_LOG"], "w") as fh:
    fh.write(args[-1] if args else "")
sys.stdout.write(os.environ.get("STUB_MODEL_OUT", ""))
sys.exit(int(os.environ.get("STUB_MODEL_EXIT", "0")))
""".format(python=sys.executable)

ENV_NAMES_TO_CLEAR = (
    "KANBAN_IMPORT_MODEL",
    "KANBAN_IMPORT_LIMIT",
    "KANBAN_IMPORT_MAX_CANDIDATES",
    "KANBAN_BIN",
    "OPENCODE_BIN",
    "XDG_RUNTIME_DIR",
)


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def write_stub(path, body):
    path.write_text(body)
    path.chmod(0o755)


def task(identifier, content=None):
    return {
        "id": identifier,
        "content": content or f"zadanie {identifier}",
        "description": "",
        "due": None,
    }


def answer(entries):
    """Build a model answer (JSON) from (id, reason) pairs or raw items."""
    return json.dumps({"import": entries}, ensure_ascii=False)


class Harness:
    """One isolated scenario: temp dir, stubs, env, log readers."""

    def __init__(self):
        location = "/tmp/opencode" if Path("/tmp/opencode").is_dir() else None
        self.tmp = tempfile.TemporaryDirectory(prefix="todoist-autoimport-", dir=location)
        self.root = Path(self.tmp.name)
        write_stub(self.root / "stub-kanban", STUB_KANBAN)
        write_stub(self.root / "stub-opencode", STUB_OPENCODE)
        env = {k: v for k, v in os.environ.items() if k not in ENV_NAMES_TO_CLEAR}
        env.update(
            {
                "XDG_RUNTIME_DIR": str(self.root),
                "KANBAN_BIN": str(self.root / "stub-kanban"),
                "OPENCODE_BIN": str(self.root / "stub-opencode"),
                "KANBAN_LOG": str(self.root / "kanban.log"),
                "OPENCODE_LOG": str(self.root / "opencode.log"),
                "PROMPT_LOG": str(self.root / "prompt.log"),
                "STUB_STATUS": json.dumps(
                    {"enabled": True, "last_sync": None, "last_error": None, "linked": 0}
                ),
                "STUB_CANDIDATES": json.dumps({"tasks": []}),
                "STUB_MODEL_OUT": answer([]),
            }
        )
        self.env = env
        self.result = None

    def set(self, **overrides):
        self.env.update({name: str(value) for name, value in overrides.items()})
        return self

    def run(self, *args):
        self.result = subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            capture_output=True,
            text=True,
            env=self.env,
            timeout=30,
        )
        return self.result

    def close(self):
        self.tmp.cleanup()

    # --- assertions / readers ------------------------------------------------

    @property
    def code(self):
        return self.result.returncode

    @property
    def stderr(self):
        return self.result.stderr

    @property
    def stdout(self):
        return self.result.stdout

    def kanban_calls(self):
        return read_calls(self.root / "kanban.log")

    def opencode_calls(self):
        return read_calls(self.root / "opencode.log")

    def imports(self):
        return [call for call in self.kanban_calls() if call[:2] == ["todoist", "import"]]

    def assert_log_lines(self):
        for line in self.stderr.splitlines():
            if line.strip():
                check(line.startswith("todoist-import: "), f"linia bez prefiksu: {line!r}")

    def assert_summary_once(self):
        hits = [line for line in self.stderr.splitlines() if "cykl kandydaci=" in line]
        check(len(hits) == 1, f"podsumowanie raz, jest {len(hits)}: {hits}")


def read_calls(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


# --- cases (§10) -------------------------------------------------------------


def case_01():
    h = Harness()
    try:
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a"), task("b")]}),
            STUB_MODEL_OUT=answer([{"id": "a", "reason": "da się"}]),
        ).run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.imports() == [["todoist", "import", "a"]], f"import: {h.kanban_calls()}")
        h.assert_log_lines()
        h.assert_summary_once()
    finally:
        h.close()


def case_02():
    h = Harness()
    try:
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT=answer(
                [{"id": "a", "reason": "ok"}, {"id": "obce", "reason": "nie ma"}]
            ),
        ).run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.imports() == [["todoist", "import", "a"]], f"import: {h.kanban_calls()}")
        check("spoza_kandydatow" in h.stderr, f"brak logu odrzucenia: {h.stderr!r}")
    finally:
        h.close()


def case_03():
    h = Harness()
    try:
        prose = 'Oto moja decyzja:\n' + answer([{"id": "a", "reason": "ok"}]) + "\nGotowe."
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT=prose,
        ).run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.imports() == [["todoist", "import", "a"]], f"import: {h.kanban_calls()}")
    finally:
        h.close()


def case_04():
    h = Harness()
    try:
        weird = 'nawiasy {} oraz "cudzysłów" i ][ — ok'
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT=answer([{"id": "a", "reason": weird}]),
        ).run("--dry-run")
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(weird in h.stderr, f"reason nie w logu: {h.stderr!r}")
        check(h.opencode_calls(), "model nie wołany")
    finally:
        h.close()


def case_05():
    h = Harness()
    try:
        h.set(STUB_CANDIDATES=json.dumps({"tasks": []})).run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.opencode_calls() == [], f"model wołany: {h.opencode_calls()}")
        check(h.imports() == [], f"import wołany: {h.kanban_calls()}")
        h.assert_summary_once()
    finally:
        h.close()


def case_06():
    h = Harness()
    try:
        h.set(
            STUB_STATUS=json.dumps(
                {"enabled": False, "last_sync": None, "last_error": None, "linked": 0}
            )
        ).run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.opencode_calls() == [], f"model wołany: {h.opencode_calls()}")
        check(h.kanban_calls() == [["todoist", "status", "--json"]], f"wołania: {h.kanban_calls()}")
    finally:
        h.close()


def case_07():
    h = Harness()
    try:
        h.set(STUB_CANDIDATES_EXIT=3).run()
        check(h.code == 1, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.opencode_calls() == [], f"model wołany: {h.opencode_calls()}")
        check(h.imports() == [], f"import wołany: {h.kanban_calls()}")
    finally:
        h.close()


def case_08():
    h = Harness()
    try:
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT=answer([{"id": "a", "reason": "ok"}]),
            STUB_MODEL_EXIT=1,
        ).run()
        check(h.code == 1, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.imports() == [], f"import wołany: {h.kanban_calls()}")
    finally:
        h.close()


def case_09():
    h = Harness()
    try:
        h.set(
            KANBAN_IMPORT_LIMIT=2,
            STUB_CANDIDATES=json.dumps({"tasks": [task("a"), task("b"), task("c")]}),
            STUB_MODEL_OUT=answer(
                [{"id": "a", "reason": ""}, {"id": "b", "reason": ""}, {"id": "c", "reason": ""}]
            ),
        ).run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.imports() == [["todoist", "import", "a", "b"]], f"import: {h.kanban_calls()}")
        check("ponad_limit" in h.stderr, f"brak logu ponad_limit: {h.stderr!r}")
    finally:
        h.close()


def case_10():
    h = Harness()
    try:
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT=answer([{"id": "a", "reason": "x"}, {"id": "a", "reason": "y"}]),
        ).run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.imports() == [["todoist", "import", "a"]], f"import: {h.kanban_calls()}")
        check("duplikat" in h.stderr, f"brak logu duplikatu: {h.stderr!r}")
    finally:
        h.close()


def case_11():
    h = Harness()
    try:
        h.set(
            KANBAN_IMPORT_MAX_CANDIDATES=2,
            STUB_CANDIDATES=json.dumps({"tasks": [task("a"), task("b"), task("c")]}),
            STUB_MODEL_OUT=answer([{"id": "a", "reason": "ok"}]),
        ).run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        prompt = (h.root / "prompt.log").read_text()
        check('"id": "a"' in prompt and '"id": "b"' in prompt, "prompt bez a/b")
        check('"id": "c"' not in prompt, "prompt zawiera zadanie ponad limit")
        check("kandydaci=3 do_promptu=2" in h.stderr, f"brak logu przycięcia: {h.stderr!r}")
    finally:
        h.close()


def case_12():
    h = Harness()
    try:
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT=answer([{"id": "a", "reason": "da się"}]),
        ).run("--dry-run")
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.imports() == [], f"import wołany: {h.kanban_calls()}")
        check("dry-run:" in h.stderr, f"brak decyzji dry-run: {h.stderr!r}")
        h.assert_summary_once()
    finally:
        h.close()


def case_13():
    h = Harness()
    handle = os.open(str(h.root / "kanban-todoist-import.lock"), os.O_WRONLY | os.O_CREAT, 0o600)
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        h.run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.kanban_calls() == [], f"CLI wołane: {h.kanban_calls()}")
        check(h.opencode_calls() == [], f"model wołany: {h.opencode_calls()}")
        check("poprzedni cykl trwa" in h.stderr, f"brak logu locka: {h.stderr!r}")
    finally:
        os.close(handle)
        h.close()


def case_14():
    h = Harness()
    try:
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT="brak json-a w tej odpowiedzi",
        ).run()
        check(h.code == 1, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.imports() == [], f"import wołany: {h.kanban_calls()}")
    finally:
        h.close()


def case_15():
    h = Harness()
    try:
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT=answer([{"id": "a", "reason": "ok"}]),
            STUB_IMPORT_OUT="raport CLI: częściowo\n",
            STUB_IMPORT_EXIT=3,
        ).run()
        check(h.code == 1, f"exit {h.code}, stderr={h.stderr!r}")
        check("raport CLI" in h.stderr, f"brak raportu CLI w logu: {h.stderr!r}")
    finally:
        h.close()


def case_16():
    h = Harness()
    try:
        secret = "TAJNE-ZDANIE-QA"
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT=answer([{"id": "a", "reason": secret}]),
        ).run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.imports() == [["todoist", "import", "a"]], f"import: {h.kanban_calls()}")
        for call in h.kanban_calls():
            check(not any(secret in arg for arg in call), f"reason w argv: {call}")
    finally:
        h.close()


def case_17():
    h = Harness()
    try:
        h.set(
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT=answer(
                [{"id": "obce", "reason": "x"}, {"id": 42, "reason": "zly"}]
            ),
        ).run()
        check(h.code == 1, f"exit {h.code}, stderr={h.stderr!r}")
        check(h.imports() == [], f"import wołany: {h.kanban_calls()}")
        check("spoza_kandydatow" in h.stderr, f"brak powodu: {h.stderr!r}")
        check("zly_ksztalt" in h.stderr, f"brak powodu: {h.stderr!r}")
    finally:
        h.close()


def case_18():
    h = Harness()
    try:
        h.set(
            STUB_STATUS=json.dumps(
                {
                    "enabled": True,
                    "last_sync": None,
                    "last_error": "401 unauthorized",
                    "linked": 0,
                }
            ),
            STUB_CANDIDATES=json.dumps({"tasks": [task("a")]}),
            STUB_MODEL_OUT=answer([{"id": "a", "reason": "ok"}]),
        ).run()
        check(h.code == 0, f"exit {h.code}, stderr={h.stderr!r}")
        check("ostrzeżenie" in h.stderr, f"brak ostrzeżenia o last_error: {h.stderr!r}")
        check(h.imports() == [["todoist", "import", "a"]], f"import: {h.kanban_calls()}")
    finally:
        h.close()


CASES = [
    ("1 model wskazuje jednego z dwóch", case_01),
    ("2 id spoza kandydatów odrzucone", case_02),
    ("3 proza + JSON w środku", case_03),
    ("4 reason z nawiasami/escape", case_04),
    ("5 pusta lista kandydatów", case_05),
    ("6 enabled=false bez wołań", case_06),
    ("7 candidates kod != 0", case_07),
    ("8 opencode kod 1", case_08),
    ("9 przycięcie do limitu", case_09),
    ("10 duplikaty id", case_10),
    ("11 limit kandydatów w promptcie", case_11),
    ("12 --dry-run bez importu", case_12),
    ("13 zajęty lock", case_13),
    ("14 odpowiedź bez JSON-a", case_14),
    ("15 import kod != 0", case_15),
    ("16 reason nie w argv", case_16),
    ("17 wszystkie id odrzucone", case_17),
    ("18 last_error + import", case_18),
]


def main():
    failures = []
    for name, case in CASES:
        try:
            case()
        except AssertionError as error:
            failures.append(name)
            print(f"FAIL {name}: {error}", file=sys.stderr)
        else:
            print(f"ok   {name}")
    if failures:
        print(f"{len(failures)}/{len(CASES)} testów nie przeszło", file=sys.stderr)
        raise SystemExit(1)
    print("OK")


if __name__ == "__main__":
    main()
