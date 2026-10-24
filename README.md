# Kanban — a local board for a human and an AI

A personal three-column kanban board: `TODO` / `In Progress` / `DONE`, with an
automatic archive. One small Rust Core (SQLite + local HTTP API), a command-line
client used by the AI, and an Omarchy bar widget used by the human. Optionally it
syncs card statuses both ways with Todoist, and an optional systemd timer lets a
model decide which Todoist tasks can be done at the computer and imports them
onto the board.

Everything runs on one machine, bound to `127.0.0.1`. There is no cloud, no
account, no web UI, and no telemetry.

The board is meant to be the shared source of truth for "what are we doing now":
the human types and drags cards, the AI adds and moves them through the CLI.

## Columns

| Column | Meaning |
|---|---|
| **TODO** | Recognised, not started. |
| **In Progress** | Current focus. |
| **DONE** | Finished. After **48 h** the card moves to the hidden `archived` status. |

`archived` is a separate status kept out of the board view. Retention is handled
by the Core; there is nothing to clean up by hand. Archived cards are still
pushed to Todoist on close, but never pulled from it.

## Architecture

```
┌───────────────────────┐        HTTP + Bearer token        ┌──────────────────────┐
│  CLI  omarchy-kanban  │ ───────────────────────────────►  │  Core (Rust, axum)   │
│  (used by the AI)     │                                   │  127.0.0.1:4765      │
└───────────────────────┘                                   │  SQLite + WAL        │
┌───────────────────────┐        JSON-lines over stdin      │  retention loop      │
│  Omarchy widget       │ ◄───────────────────────────────► │  Todoist sync (60 s) │
│  (Quickshell/QML)     │        omarchy-kanban bridge       └──────────┬───────────┘
└───────────────────────┘                                              │ HTTPS
┌───────────────────────┐                                              ▼
│  auto-import timer    │ ── candidates/import ──► CLI        Todoist REST API v1
│  (systemd user)       │ ── prompt ──► opencode run
└───────────────────────┘
```

| Component | File | Role |
|---|---|---|
| Core | `src/board.rs` | SQLite schema, CRUD, HTTP API, retention, Todoist endpoints and loop |
| Todoist client | `src/todoist.rs` | REST v1 client, push/pull cycle, backoff, conflict resolution |
| CLI | `src/main.rs` | `task` and `todoist` commands over the local HTTP API |
| Widget bridge | `src/bridge.rs` | JSON-lines adapter used by the QML widget |
| Widget | `plugin/Widget.qml` | Omarchy bar widget |
| Installer | `install.py` | Builds and installs the binary, widget, units, auto-import |
| Token helper | `todoist-token.py` | Stores the Todoist token, adds a systemd drop-in |
| Auto-import | `todoist-autoimport.py` | Timer-driven orchestration of candidate → model → import |
| AI skill | `skill/kanban-triage/SKILL.md` | Tells an AI agent when and how to use the board |

## Requirements

- Linux with systemd user services (the installer is Linux-only; the Core code
  also builds on other Unixes but there is no service management).
- Rust / Cargo ≥ 1.89 to build from source.
- Omarchy / Quickshell for the bar widget (optional; the CLI and API work
  without it).
- `opencode` for the optional AI auto-import.

No Python packages are needed: every helper script uses the standard library
only. The Rust build adds no crates for any of this — `Cargo.toml` and
`Cargo.lock` are unchanged by the Todoist work.

## Install / update

```sh
python3 install.py --yes              # build from source, install Core + widget + auto-import timer
python3 install.py --no-widget --yes  # Core only
python3 install.py --no-timer --yes   # skip the auto-import timer (disables an existing one)
python3 install.py --help
```

The installer:

1. builds `omarchy-kanban` and installs it to `~/.local/bin/omarchy-kanban`,
2. backs up the database, runs the additive schema migration, enables and
   restarts `agent-kanban.service`,
3. installs the widget plugin into `~/.config/omarchy/plugins/bart.kanban`
   (backing up user changes to `Widget.qml` and `shell.json`),
4. installs `omarchy-kanban-todoist-import` to `~/.local/bin` and the
   `agent-kanban-todoist-import.{service,timer}` units, then reloads systemd,
   starts the timer and enables lingering so the timer also fires while logged
   out (`--no-timer` skips the units and disables an existing timer).

Re-running the installer is safe: it preserves the database, the Todoist
drop-in and the widget placement.

## Widget

The widget shows a single icon; counts are in the tooltip. Clicking opens the
three columns.

- **Add**: hover a column and a "＋ New task" row appears at its bottom; click it
  to open the editor. Hovering an existing card hides the row.
- **Move**: drag the whole card. The target column highlights and the card is
  inserted where you dropped it. Dropping outside a column changes nothing.
- **Details**: the `⋯` icon on a card opens the editor (title, note, **Save** /
  **Cancel**). The note is shown on the card.
- Accents: a coloured status bar per card, dots in headers, a top accent, and
  `hand`/`grab` cursors on clickable elements.

## CLI

The CLI is what the AI drives. `<id>` may be any unique prefix of the UUID.

```sh
omarchy-kanban task list [--all] [--json]     # three columns (+ hidden archive)
omarchy-kanban task add "Title" [--status todo|doing|done] [--notes "…"] [--json]
omarchy-kanban task move <id> todo|doing|done
omarchy-kanban task edit <id> [--title "…"] [--notes "…"]
omarchy-kanban task rm <id>

omarchy-kanban todoist status [--json]        # enabled, last_sync, last_error, linked
omarchy-kanban todoist candidates [--json]    # due today or overdue, not yet on the board
omarchy-kanban todoist import <id>… [--status todo|doing|done]
omarchy-kanban todoist sync                   # force one synchronisation cycle

omarchy-kanban serve [--port <port>]          # run the Core in the foreground
omarchy-kanban bridge                         # JSON-lines client for the widget
```

The CLI talks to the local Core at `http://127.0.0.1:4765` using the token from
the data directory. It never talks to Todoist directly.

## HTTP API

Every request needs `Authorization: Bearer <token>` (token:
`~/.local/share/agent-kanban/token`) and a loopback `Host` header.

| Method | Path | Body |
|---|---|---|
| GET | `/api/state` | — (whole board plus counters) |
| POST | `/api/tasks` | `{"title","notes?","status?"}` |
| PATCH | `/api/tasks/{id}` | `{"title?","notes?","status?","position?"}` |
| DELETE | `/api/tasks/{id}` | — |
| GET | `/api/todoist/status` | — |
| GET | `/api/todoist/candidates` | — |
| POST | `/api/todoist/import` | `{"ids":[<id>…],"status":"todo\|doing\|done"}` |
| POST | `/api/todoist/sync` | — |

## Todoist integration (optional)

Set the token with the helper; it never appears in the repo, the unit file, the
process arguments or the logs:

```sh
python3 todoist-token.py            # paste a token from Todoist → Settings → Integrations
python3 todoist-token.py --dry-run  # show what would be written, change nothing
python3 todoist-token.py --remove   # disable the integration
```

The script validates the token against the API, writes
`$XDG_CONFIG_HOME/agent-kanban/todoist.env` (`0600`, directory `0700`) and adds the
systemd drop-in `agent-kanban.service.d/10-todoist.conf` pointing at that file via
`EnvironmentFile=`. Then it restarts the Core. Without `TODOIST_API_TOKEN` the
integration is simply off and the board works as usual.

What the integration does:

- **Two-way statuses.** Moving a card to `DONE` closes the Todoist task. Completing
  a task in Todoist moves the card to `DONE`. Pulling a card back out of `DONE`
  reopens the task. The Core pushes first and pulls second, every 60 s; force it
  with `omarchy-kanban todoist sync`.
- **A manual change always wins.** Status writes are compare-and-set against
  `updated_at`, so a card edited after the last sync is never overwritten by a
  remote completion.
- **Archive is push-only.** An archived card is closed in Todoist, but a remote
  completion never resurrects or moves an archived card.
- **Deletion stays local.** `task rm` removes the card only; the Todoist task is
  left untouched.
- **Not synced:** titles, notes, labels, projects, priority and dates. Only
  statuses travel. Also: a task completed in Todoist and reopened there does not
  revive a `DONE` card, and a recurring task's next occurrence never appears on the
  board automatically (its old card stays `DONE`).
- **API limits:** REST v1, 1000 requests / 15 min. Completion queries are capped at
  89 days, and the pull window is clamped so a backwards clock jump cannot produce
  an infinite `400` loop.

State and errors: `omarchy-kanban todoist status` (`enabled`, `last_sync`,
`last_error`, number of linked cards). `last_error` comes from the last cycle and
is cleared by the next successful one.

## AI auto-import (optional)

The Core never imports on its own. A systemd user timer outside the Core does the
judgement call hourly from 07:00 through 23:00 in the machine's local time:

```
agent-kanban-todoist-import.timer  (OnCalendar=*-*-* 07..23:00:00, Persistent=true)
  └─ agent-kanban-todoist-import.service  (Type=oneshot, TimeoutStartSec=15min)
       └─ ~/.local/bin/omarchy-kanban-todoist-import
            ├─ omarchy-kanban todoist status --json      (skip when disabled)
            ├─ omarchy-kanban todoist candidates --json  (due today + overdue)
            ├─ opencode run --standalone --model <model> <fixed prompt>
            ├─ validate the answer against the candidate list
            └─ omarchy-kanban todoist import <ids…>
```

The task of the model is narrow: decide which tasks can be done sitting at a
computer (writing, coding, email, online orders, forms, research, calls,
payments, bookings) and which are physical (cleaning, painting, repairs,
errands). The prompt is a fixed text in the script and is part of the contract;
only the task list is substituted.

The model's answer is treated as untrusted input:

- only the first JSON object in the response is parsed (`{"import":[{"id","reason"?}]}`),
- every `id` must be a string that was sent in the same prompt,
- duplicates collapse, unknown or malformed entries are rejected and logged,
- the accepted list is capped (10 per cycle by default),
- `reason` never reaches the Core or the card; it is only logged.

If systemd catches up a missed run at night, the script exits before calling the
CLI or model; the next scheduled run is 07:00. `Persistent=true` triggers one
catch-up after a missed timer event, not a backlog of every missed hourly tick.

Exit codes are meaningful for `systemctl --user status`: `0` for a normal cycle
(nothing to do, integration off, or imports done), non-zero for a broken cycle
(Core unreachable, model error, no parseable answer, import failure). Failures
are logged to journald only — there are no desktop notifications by design.

Configuration lives in the unit and can be overridden with a drop-in
(`systemctl --user edit agent-kanban-todoist-import.service`):

| Variable | Default | Meaning |
|---|---|---|
| `KANBAN_IMPORT_MODEL` | `opencode/glm-5.3-flash-free` | model passed to `opencode run` |
| `KANBAN_IMPORT_LIMIT` | `10` | maximum imports per cycle |
| `KANBAN_IMPORT_MAX_CANDIDATES` | `100` | maximum candidates sent in the prompt |
| `KANBAN_BIN` / `OPENCODE_BIN` | from `PATH` | binary overrides (tests, unusual installs) |

Inspect or try it:

```sh
omarchy-kanban-todoist-import --dry-run                          # decision only, no import
systemctl --user start agent-kanban-todoist-import.service       # one real cycle
journalctl --user -u agent-kanban-todoist-import -n 50 --no-pager
systemctl --user list-timers agent-kanban-todoist-import.timer
```

The model is non-deterministic and `opencode run` exposes neither `temperature`
nor a seed: stability comes from the fixed prompt and a pinned model, not from
repeatable sampling. The default is a free model; switch `KANBAN_IMPORT_MODEL`
to change it.

## AI skill

`kanban-triage` tells an AI agent when a piece of work deserves a card, which
column it starts in, and how to drive the CLI. It is installed to
`~/.config/opencode/skills/`; the source is `skill/kanban-triage/SKILL.md`.

## Files on disk

| Path | Contents |
|---|---|
| `~/.local/share/agent-kanban/board.sqlite3` | the board (WAL) |
| `~/.local/share/agent-kanban/token` | Core bearer token (`0600`) |
| `~/.local/share/agent-kanban/identity.json` | Core id, protocol version, port |
| `~/.local/share/agent-kanban/backups/` | database and config backups made by the installer |
| `~/.config/agent-kanban/todoist.env` | Todoist token (`0600`, read only through `EnvironmentFile=`) |
| `~/.config/systemd/user/agent-kanban.service` | Core unit |
| `~/.config/systemd/user/agent-kanban-todoist-import.{service,timer}` | auto-import units |
| `~/.config/systemd/user/agent-kanban.service.d/10-todoist.conf` | Todoist token drop-in |

Logs: `journalctl --user -u agent-kanban.service -n 100 --no-pager`.

## Tests

```sh
cargo test --all-targets --locked        # unit tests: schema, sync rules, client, conflicts
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
cargo build --release --locked

python3 tests/todoist_e2e.py             # process test: import → close → reopen → pull (fake Todoist)
python3 tests/todoist_autoimport.py      # 25 scenarios with stubbed CLI and model, no network
python3 tests/todoist_badge.py           # freshness predicate from QML, executed in Node
python3 tests/install_test.py            # installer sandbox: units, timer, tokens, backups
```

The Python tests never touch a live service, a real Todoist account or a model.
`tests/todoist_autoimport.py` asserts, among other things, that the token cannot
leak into arguments or logs, and that a hallucinated task id is rejected.

## Design notes

- **Sync is deterministic; judgement is not.** The Core only moves statuses. Any
  decision that needs judgement (is this task doable at a computer?) stays outside
  the Core, in the CLI or the auto-import script.
- **No new dependencies.** The scripts use the Python standard library; the Rust
  side is unchanged in `Cargo.toml`/`Cargo.lock`.
- **Local trust boundary.** The Core listens on loopback only, checks the `Host`
  header and requires a bearer token from a `0600` file. The Todoist token is
  passed only through the service environment.
- **Additive migrations.** Schema changes add columns and indexes; existing rows
  are preserved. Each column is checked separately so an interrupted migration
  can be resumed.

Specifications and their HTML previews live in `docs/specs/`:

- `todoist-sync.md` / `.html` — two-way status sync, conflict rules, token handling
- `todoist-autoimport.md` / `.html` — timer, model contract, validation, failure modes

## Known limitations

- Statuses only; title/note changes are not synchronised with Todoist.
- Recurring Todoist tasks return with a new id, so a new occurrence is not picked
  up automatically; it appears after the next `todoist candidates`/auto-import run.
- One machine and one user; no remote Core, no multi-device merge.
- The auto-import decision is non-deterministic and depends on a free model's
  availability and rate limits.
- The widget requires Omarchy/Quickshell; the CLI and HTTP API do not.
- `web/`, `README-legacy.md` and parts of `tests/` come from the previous version
  of the project (an OpenCode session observer) and are unused.
