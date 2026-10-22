#!/usr/bin/env python3
"""Deterministic ACP test peer. Never invokes a model or modifies project files."""
import json
import sys
import uuid

session = "ses_fake_" + uuid.uuid4().hex
active = None
pending = {}


def emit(value):
    print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)


def result(identifier, value):
    emit({"id": identifier, "result": value})


def finish(identifier, text="Test result verified.", reason="end_turn"):
    emit({"method": "session/update", "params": {"sessionId": session, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}}})
    result(identifier, {"stopReason": reason})


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    identifier = request.get("id")
    params = request.get("params", {})
    if method == "initialize":
        result(identifier, {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}})
    elif method in ("session/new", "session/load"):
        session = params.get("sessionId", session)
        result(identifier, {"sessionId": session, "configOptions": [{"id": "mode", "type": "select", "currentValue": "plan", "options": [{"value": "plan", "name": "Plan"}, {"value": "build", "name": "Build"}]}]})
    elif method in ("session/set_mode", "session/set_config_option"):
        result(identifier, {})
    elif method == "session/prompt":
        active = identifier
        text = params["prompt"][0]["text"]
        emit({"method": "session/update", "params": {"sessionId": session, "update": {"sessionUpdate": "tool_call", "toolCallId": "read-1", "title": "Read test fixture", "status": "in_progress"}}})
        if "TEST_CRASH" in text:
            sys.exit(9)
        elif "TEST_PERMISSION" in text:
            pending["permission-1"] = identifier
            emit({"id": "permission-1", "method": "session/request_permission", "params": {"sessionId": session, "toolCall": {"toolCallId": "read-1", "title": "Inspect fixture"}, "options": [{"optionId": "once", "kind": "allow_once", "name": "Allow once"}, {"optionId": "reject", "kind": "reject_once", "name": "Reject"}]}})
        elif "TEST_FORM" in text:
            pending["form-1"] = identifier
            emit({"id": "form-1", "method": "elicitation/create", "params": {"mode": "form", "sessionId": session, "message": "Which environment?", "requestedSchema": {"type": "object", "properties": {"environment": {"type": "string", "enum": ["test", "production"]}}, "required": ["environment"], "additionalProperties": False}}})
        elif "TEST_WAIT" in text:
            pass
        else:
            finish(identifier)
    elif method == "session/cancel":
        if active is not None:
            finish(active, "Cancelled", "cancelled")
            active = None
    elif identifier in pending:
        prompt_id = pending.pop(identifier)
        finish(prompt_id, "Decision received: " + json.dumps(request.get("result")))
    elif method:
        emit({"id": identifier, "error": {"code": -32601, "message": "Unknown test method"}})
