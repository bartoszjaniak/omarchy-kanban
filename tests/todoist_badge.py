#!/usr/bin/env python3
import re
import subprocess
from pathlib import Path


widget_qml = Path(__file__).resolve().parents[1] / "plugin" / "Widget.qml"
source = widget_qml.read_text(encoding="utf-8")
match = re.search(
    r"function\s+taskIsFreshImport\s*\([^)]*\)\s*\{(?P<body>[^{}]*)\}",
    source,
)
if not match:
    raise SystemExit(f"Could not find taskIsFreshImport body in {widget_qml}")

node_test = r"""
const assert = require('node:assert/strict');
const fs = require('node:fs');
const body = fs.readFileSync(0, 'utf8');
const taskIsFreshImport = new Function('task', 'clockNowMs', body);
const now = 1_000_000;
const createdAt = Math.floor(now / 1000);

assert.equal(taskIsFreshImport({ todoist_id: 'linked', created_at: createdAt }, now), true, 'age 0');
assert.equal(taskIsFreshImport({ todoist_id: 'linked', created_at: createdAt - 86399 }, now), true, 'age 86399');
assert.equal(taskIsFreshImport({ todoist_id: 'linked', created_at: createdAt - 86400 }, now), false, 'age 86400');
assert.equal(taskIsFreshImport({ todoist_id: 'linked', created_at: createdAt + 1 }, now), false, 'future created_at');
assert.equal(taskIsFreshImport({ todoist_id: null, created_at: createdAt }, now), false, 'unlinked task');
"""

try:
    subprocess.run(
        ["node", "-e", node_test],
        input=match.group("body"),
        text=True,
        check=True,
    )
except FileNotFoundError:
    raise SystemExit("Node.js is required to run this test")

print("OK")
