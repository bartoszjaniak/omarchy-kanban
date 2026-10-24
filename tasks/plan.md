# Implementation Plan: Todoist daytime hourly auto-import

## Overview

Change the AI auto-import timer from every two hours around the clock to hourly
at 07:00–23:00 local time. Preserve systemd's single missed-run catch-up after
boot/resume, but make the script skip a catch-up if it fires overnight.

## Decisions

- Keep the existing systemd user timer and `Persistent=true`; do not add a second
  `OnBootSec` trigger, which could duplicate runs.
- Use `OnCalendar=*-*-* 07..23:00:00`; verified with `systemd-analyze calendar`.
- Guard the script before lock/CLI calls so a persistent catch-up during
  00:00–06:59 logs and exits 0. The next scheduled run is 07:00.
- Todoist status sync in Core remains every 60 seconds and is unchanged.

## Tasks

1. Script time-window guard + regression test (`todoist-autoimport.py`,
   `tests/todoist_autoimport.py`).
2. Timer schedule + installer test (`install.py`, `tests/install_test.py`).
3. Update README/spec preview; run all gates and install/reload timer.

## Risks

- OpenCode free-model quota was exhausted at 10:07 and 12:06 on 2026-10-24;
  more frequent attempts may fail more often until quota resets or model changes.
- Todoist candidates request timed out at 14:33; this is separate from timer
  cadence and remains logged by the script.
- systemd calendar uses the machine's local timezone.

## Verification

- `systemd-analyze calendar '*-*-* 07..23:00:00'`
- `python3 tests/todoist_autoimport.py`
- `python3 tests/install_test.py`
- `cargo fmt --check`, `cargo test --all-targets --locked`,
  `cargo clippy --all-targets --locked -- -D warnings`,
  `cargo build --release --locked`, `python3 tests/todoist_e2e.py`
- Check timer and journald on the live user service after installation.
