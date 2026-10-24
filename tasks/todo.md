# Tasks: Todoist daytime hourly auto-import

- [ ] Script skips out-of-window invocations before CLI/model; add test.
- [ ] Change systemd OnCalendar to hourly 07:00–23:00; update installer test.
- [ ] Update English README, regenerate HTML preview, run gates and install.

## Checkpoint

- [ ] Nighttime catch-up exits without CLI/model; next scheduled run is 07:00.
- [ ] Missed timer catch-up remains enabled via `Persistent=true`.
- [ ] Tests/build/review pass; live timer reports new calendar schedule.
