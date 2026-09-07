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

if os.environ.get("TEST_PAUSE_WORKER") and not prompt.startswith("You are a note-taker"):
    while not Path("release.worker").exists():
        time.sleep(0.02)
if os.environ.get("TEST_PAUSE_HELPER") and prompt.startswith("You are a note-taker"):
    while not Path("release.helper").exists():
        time.sleep(0.02)

def emit(event):
    print(json.dumps(event), flush=True)

if "depleted" in mode:
    count = len(Path("calls.jsonl").read_text().splitlines())
    matches = mode.startswith(agent) or mode.startswith("both")
    helper = "helper" not in mode or prompt.startswith("You are a note-taker")
    through = int(os.environ.get("TEST_DEPLETED_CALLS", "2" if mode.startswith("both") else "100"))
    if matches and helper and count <= through:
        message = "You've hit your limit" if agent == "claude" else "insufficient_quota: You exceeded your current quota"
        message = os.environ.get("TEST_" + agent.upper() + "_LIMIT_MESSAGE", message)
        if os.environ.get("TEST_LIMIT_STDERR"):
            print(os.environ["TEST_LIMIT_STDERR"], file=sys.stderr)
        if "stderr" in mode:
            print(message, file=sys.stderr)
        elif agent == "codex":
            emit({"type": "turn.failed", "error": {"message": message, "status_code": 429}})
        else:
            emit({"type": "result", "is_error": True, "result": message, "total_cost_usd": 0.125})
        sys.exit(1)
    mode = os.environ.get("TEST_SUCCESS_MODE", "complete")

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

answer = "RALPH_COMPLETE" if mode in ("complete", "commit-complete", "queue-complete", "switch-complete", "queue-add-complete") else "reviewed the next task"
if prompt.startswith("You are a note-taker"):
    answer = "- carry this constraint forward"
elif prompt.startswith("You are an adversarial reviewer"):
    answer = os.environ.get("TEST_REVIEW_RESULT", "REFUTE: verification did not cover the change")
elif prompt.startswith("You mine an autonomous"):
    answer = "[]"
elif mode in ("commit", "commit-complete", "queue-complete", "incremental-commit"):
    Path("product.txt").write_text("implemented " + str(len(Path("calls.jsonl").read_text().splitlines())) + "\n")
    subprocess.run(["git", "add", "product.txt"], check=True)
    subprocess.run(["git", "commit", "-qm", "Implement task"], check=True)
    if mode == "incremental-commit":
        pass
    elif mode == "queue-complete":
        subprocess.run([os.environ["TEST_RALPH_BIN"], "done", "1"], check=True, stdout=subprocess.DEVNULL)
    else:
        backlog = Path(".ralph/BACKLOG.md")
        backlog.write_text(backlog.read_text().replace("- [ ]", "- [x]"))
elif mode == "switch-complete":
    subprocess.run(["git", "checkout", "-qb", "wrong-branch"], check=True)
elif mode == "queue-add-complete":
    subprocess.run([os.environ["TEST_RALPH_BIN"], "add", "Follow-up", "--verify", "looks clearer"], check=True, stdout=subprocess.DEVNULL)
elif mode == "review":
    Path(".ralph/HANDOFF.json").write_text(json.dumps({"status": "review", "model": None, "blocked": None}))

if os.environ.get("TEST_PAUSE_AFTER_CLOSURE") and mode in ("queue-complete", "commit-complete"):
    Path("closure.ready").touch()
    while not Path("release.closure").exists():
        time.sleep(0.02)

if os.environ.get("TEST_WEAKEN_CONTRACT") and answer == "RALPH_COMPLETE":
    backlog = Path(".ralph/BACKLOG.md")
    backlog.write_text(backlog.read_text().replace("Verify: `true` exits 0.", "Verify: trust the summary."))

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
