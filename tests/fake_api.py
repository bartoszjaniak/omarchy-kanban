#!/usr/bin/env python3
"""Read-only shared OpenCode API fixture. Rejects ALL non-GET and execution calls."""
import json
import os
import sys
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

if len(sys.argv) != 4 or sys.argv[1:3] != ["api", "GET"]:
    raise SystemExit("Fixture forbids prompts, ACP and mutating OpenCode calls")
if os.environ.get("KANBAN_FAKE_API_LOG"):
    with open(os.environ["KANBAN_FAKE_API_LOG"], "a") as log:
        log.write(json.dumps(sys.argv[1:]) + "\n")
data = json.loads(Path(os.environ["KANBAN_FIXTURE"]).read_text())
if data.get("offline"):
    raise SystemExit(7)
url = urlsplit(sys.argv[3])
query = parse_qs(url.query)
sessions = data.get("sessions", [])
if url.path == "/api/session":
    roots = [s for s in sessions if not s.get("parentID")]
    offset = int(query.get("cursor", ["0"])[0])
    page = roots[offset:offset + 2]
    result = {"data": page, "cursor": {"next": str(offset + 2) if offset + 2 < len(roots) else None}}
elif url.path == "/api/session/active":
    result = {"data": {s: {"type": "running"} for s in data.get("active", [])}}
else:
    parts = url.path.strip("/").split("/")
    if len(parts) not in (3, 4) or parts[:2] != ["api", "session"]:
        raise SystemExit("Unsupported read route")
    session = next((s for s in sessions if s["id"] == parts[2]), None)
    if not session:
        raise SystemExit(4)
    if len(parts) == 3:
        result = {"data": session}
    elif parts[3] in ("form", "permission"):
        result = {"data": data.get(parts[3], {}).get(parts[2], [])}
    else:
        raise SystemExit("Unsupported read route")
print(json.dumps(result))
