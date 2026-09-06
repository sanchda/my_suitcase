#!/usr/bin/python3
"""Offline CLI protocol fixture. Never invokes a model or reads credentials."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

args = sys.argv[1:]
prompt = sys.stdin.read()
agent = Path(sys.argv[0]).name
mode = os.environ.get("TEST_AGENT_MODE", "complete")
with Path("calls.jsonl").open("a") as log:
    log.write(json.dumps({"agent": agent, "args": args, "prompt": prompt}) + "\n")

def emit(event):
    print(json.dumps(event), flush=True)

if mode == "hang":
    child = subprocess.Popen(["/bin/sleep", "60"])
    Path("child.pid").write_text(str(child.pid))
    time.sleep(60)

if mode == "stderr-error":
    print("error: unexpected argument '--bad-option' found", file=sys.stderr)
    sys.exit(2)

if mode in ("limit-retry", "transient-retry"):
    if len(Path("calls.jsonl").read_text().splitlines()) == 1:
        emit({"type": "turn.failed", "error": {"message": "retry later", "status_code": 429 if mode == "limit-retry" else 503}})
        sys.exit(1)
    mode = "complete"

if mode == "fatal":
    if agent == "codex":
        emit({"type": "turn.failed", "error": {"message": "model is not supported", "status_code": 400}})
    else:
        emit({"type": "result", "is_error": True, "result": "model does not exist"})
    sys.exit(0)  # Some CLIs report API errors with a successful process exit.

answer = "RALPH_COMPLETE" if mode == "complete" else "reviewed the next task"
if prompt.startswith("You are a note-taker"):
    answer = "- carry this constraint forward"
elif prompt.startswith("You are an adversarial reviewer"):
    answer = "REFUTE: verification did not cover the change"
elif prompt.startswith("You mine an autonomous"):
    answer = "[]"
elif mode == "commit":
    Path("product.txt").write_text("implemented\n")
    subprocess.run(["git", "add", "product.txt"], check=True)
    subprocess.run(["git", "commit", "-qm", "Implement task"], check=True)
    backlog = Path(".ralph/BACKLOG.md")
    backlog.write_text(backlog.read_text().replace("- [ ]", "- [x]"))
elif mode == "review":
    Path(".ralph/HANDOFF.json").write_text(json.dumps({"status": "review", "model": None, "blocked": None}))

if agent == "claude":
    if "text" in args:
        print(answer)
    else:
        emit({"type": "result", "is_error": False, "result": answer, "total_cost_usd": 0.25})
else:
    emit({"type": "thread.started", "thread_id": "11111111-2222-3333-4444-555555555555"})
    emit({"type": "turn.started"})
    emit({"type": "item.started", "item": {"id": "1", "type": "command_execution", "command": "true"}})
    emit({"type": "item.completed", "item": {"id": "1", "type": "command_execution", "aggregated_output": "RALPH_COMPLETE"}})
    emit({"type": "item.completed", "item": {"id": "2", "type": "reasoning", "text": "RALPH_COMPLETE"}})
    emit({"type": "item.completed", "item": {"id": "3", "type": "agent_message", "text": answer}})
    emit({"type": "turn.completed", "usage": {"input_tokens": 100, "cached_input_tokens": 60, "output_tokens": 12}})
