#!/usr/bin/env bash
# Ralph external loop — fresh-context iterations of `claude -p` until a completion
# marker appears, robust against usage/credit exhaustion. See README.md.
#
# Each iteration is a NEW `claude` process (fresh context) piped the same PROMPT.md.
# Cross-iteration memory lives in files (PROGRESS.md), not context — this keeps
# per-call cost bounded even on context-expensive thinking models.
#
# This is the GLOBAL, project-agnostic runner (installed via the suitcase `bin/`
# as `ralph`). It is driven entirely by files in the current repo — the prompt,
# north-star, backlog, and progress log are all LOCAL and supplied per project.
# Nothing project-specific lives in this script.
set -uo pipefail

# --- config (env overridable; flags win) ---
RALPH_DIR="${RALPH_DIR:-.ralph}"
RALPH_PROMPT="${RALPH_PROMPT:-tools/ralph/PROMPT.md}"
RALPH_MODEL="${RALPH_MODEL:-sonnet}"               # default tier; per-iteration override via $RALPH_DIR/MODEL
RALPH_FALLBACK_MODEL="${RALPH_FALLBACK_MODEL:-sonnet}"  # CLI auto-falls-back when overloaded
RALPH_MAX_ITER="${RALPH_MAX_ITER:-0}"             # 0 = unlimited
RALPH_MARKER="${RALPH_MARKER:-RALPH_COMPLETE}"    # final-text token that ends the loop
RALPH_YOLO="${RALPH_YOLO:-1}"                     # 1 = --dangerously-skip-permissions (needed for unattended)
RALPH_OUTPUT_FORMAT="${RALPH_OUTPUT_FORMAT:-stream-json}"  # stream-json log captures thinking blocks
RALPH_LIMIT_WAIT="${RALPH_LIMIT_WAIT:-300}"       # base backoff for usage/rate limit (s)
RALPH_LIMIT_WAIT_MAX="${RALPH_LIMIT_WAIT_MAX:-3600}"
RALPH_TRANSIENT_WAIT="${RALPH_TRANSIENT_WAIT:-10}"     # base backoff for 5xx/network (s)
RALPH_TRANSIENT_WAIT_MAX="${RALPH_TRANSIENT_WAIT_MAX:-300}"
RALPH_EXTRA_ARGS="${RALPH_EXTRA_ARGS:-}"          # extra flags passed through to claude verbatim

ONCE=0
usage() {
  sed -n '2,4p' "$0"
  cat <<'EOF'

Usage: ralph [options]   (run from the repo root you want the loop to work in)
  --prompt <file>          Prompt file fed each iteration (default tools/ralph/PROMPT.md)
  --model <name>           Default model tier (default sonnet)
  --fallback-model <name>  Overloaded-fallback model (default sonnet; "" to disable)
  --max-iterations <n>     Stop after n iterations (default 0 = unlimited)
  --marker <text>          Final-text token that ends the loop (default RALPH_COMPLETE)
  --dir <path>             Runtime/log dir (default .ralph)
  --once                   Run a single iteration then exit (for testing)
  --no-yolo                Do NOT pass --dangerously-skip-permissions (loop will stall on prompts)
  -h, --help               This help

Control while running:
  touch .ralph/STOP        Ask the loop to halt after the current iteration
  tail -f .ralph/current.log   Watch the active iteration live (includes thinking)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prompt) RALPH_PROMPT="$2"; shift 2;;
    --model) RALPH_MODEL="$2"; shift 2;;
    --fallback-model) RALPH_FALLBACK_MODEL="$2"; shift 2;;
    --max-iterations) RALPH_MAX_ITER="$2"; shift 2;;
    --marker) RALPH_MARKER="$2"; shift 2;;
    --dir) RALPH_DIR="$2"; shift 2;;
    --once) ONCE=1; shift;;
    --no-yolo) RALPH_YOLO=0; shift;;
    -h|--help) usage; exit 0;;
    *) echo "unknown arg: $1" >&2; usage; exit 2;;
  esac
done

command -v claude >/dev/null || { echo "ralph: claude CLI not found" >&2; exit 3; }
command -v jq >/dev/null     || { echo "ralph: jq not found (required)" >&2; exit 3; }
[[ -f "$RALPH_PROMPT" ]]     || { echo "ralph: prompt file not found: $RALPH_PROMPT" >&2; exit 3; }

mkdir -p "$RALPH_DIR/logs"
ITER_FILE="$RALPH_DIR/iteration"
STOP_FILE="$RALPH_DIR/STOP"
RUN_LOG="$RALPH_DIR/run.log"
[[ -f "$ITER_FILE" ]] || echo 0 > "$ITER_FILE"

log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*" | tee -a "$RUN_LOG"; }
trap 'log "interrupted (SIGINT) — exiting"; exit 130' INT TERM

# The prompt asks each iteration to commit its own work. Warn (don't act) if the
# tracked tree got NEW dirt vs. the loop-start baseline — pre-existing operator
# dirt (WIP files, submodule drift) must not cry wolf every iteration.
git_baseline() {
  git -C "$PWD" rev-parse --git-dir >/dev/null 2>&1 || return 0
  git status --porcelain --untracked-files=no 2>/dev/null | sort > "$RALPH_DIR/git-baseline"
}
git_dirty_warn() {
  [[ -f "$RALPH_DIR/git-baseline" ]] || return 0
  local new
  new=$(git status --porcelain --untracked-files=no 2>/dev/null | sort \
    | comm -13 "$RALPH_DIR/git-baseline" - | wc -l)
  [[ "$new" -gt 0 ]] && log "  ⚠ $new newly-dirty tracked file(s) after iter — agent may have skipped its commit"
  return 0
}

# claude arg array (model resolved per-iteration — see resolve_model)
claude_args=( -p --output-format "$RALPH_OUTPUT_FORMAT" )
[[ "$RALPH_OUTPUT_FORMAT" == "stream-json" ]] && claude_args+=( --verbose )
[[ "$RALPH_YOLO" == "1" ]] && claude_args+=( --dangerously-skip-permissions )
# shellcheck disable=SC2206
[[ -n "$RALPH_EXTRA_ARGS" ]] && claude_args+=( $RALPH_EXTRA_ARGS )

# Per-iteration model tier: the previous iteration (or the operator) may write
# haiku|sonnet|opus into $RALPH_DIR/MODEL to size the NEXT step's model.
# Unknown values are ignored with a warning (a typo must not 404-abort the loop).
resolve_model() {
  local m=""
  [[ -f "$RALPH_DIR/MODEL" ]] && m=$(tr -d '[:space:]' < "$RALPH_DIR/MODEL")
  case "$m" in
    haiku|sonnet|opus) echo "$m";;
    "") echo "$RALPH_MODEL";;
    *) log "  ⚠ ignoring invalid $RALPH_DIR/MODEL ('$m') — using $RALPH_MODEL" >&2; echo "$RALPH_MODEL";;
  esac
}

# classify(is_error, api_status, result_text) -> SUCCESS|LIMIT|TRANSIENT|FATAL
classify() {
  local is_err="$1" status="$2" t
  t=$(printf '%s' "$3" | tr '[:upper:]' '[:lower:]')
  [[ "$is_err" != "true" ]] && { echo SUCCESS; return; }
  # usage / credit exhaustion — the condition we must survive by waiting it out
  if [[ "$t" =~ (usage[[:space:]]+limit|credit[[:space:]]+balance|out[[:space:]]+of[[:space:]]+credit|quota|insufficient[[:space:]]+(credit|quota|funds)|resets?[[:space:]]+at|will[[:space:]]+reset|too[[:space:]]+many[[:space:]]+requests|rate[[:space:]]+limit) ]]; then
    echo LIMIT; return; fi
  case "$status" in
    429) echo LIMIT; return;;
    500|502|503|504|529) echo TRANSIENT; return;;
    401|403) echo FATAL; return;;
    400|404) echo FATAL; return;;
  esac
  if [[ "$t" =~ (overloaded|internal[[:space:]]+server|timeout|timed[[:space:]]+out|connection|network|econnreset|socket|temporarily) ]]; then
    echo TRANSIENT; return; fi
  if [[ "$t" =~ (invalid[[:space:]]+api[[:space:]]+key|authentication|no[[:space:]]+access|does[[:space:]]+not[[:space:]]+exist|it[[:space:]]+may[[:space:]]+not[[:space:]]+exist) ]]; then
    echo FATAL; return; fi
  echo TRANSIENT  # default: retry rather than die
}

# run_one(iter) -> rc: 0 advance | 1 retry-limit | 2 complete | 3 abort-fatal | 4 retry-transient
run_one() {
  local n="$1" ts logf envelope is_err status text cost class model fb
  model=$(resolve_model)
  fb="$RALPH_FALLBACK_MODEL"; [[ "$fb" == "$model" ]] && fb=""
  ts=$(date -u +%Y%m%dT%H%M%SZ)
  logf="$RALPH_DIR/logs/iter-$(printf '%04d' "$n")-$ts.log"
  ln -sf "logs/$(basename "$logf")" "$RALPH_DIR/current.log"
  log "iter $n → $model"
  # exit code is unreliable (CLI returns 0 on API errors) — parse the envelope instead
  claude "${claude_args[@]}" --model "$model" ${fb:+--fallback-model "$fb"} \
    < "$RALPH_PROMPT" > "$logf" 2>&1 || true
  envelope=$(grep '"type":"result"' "$logf" | tail -n1)
  if [[ -z "$envelope" ]]; then
    log "  no result envelope (crash/kill/empty output) → transient"; return 4; fi
  printf '%s\n' "$envelope" > "$RALPH_DIR/last-result.json"
  is_err=$(printf '%s' "$envelope" | jq -r '.is_error // false')
  status=$(printf '%s' "$envelope" | jq -r '.api_error_status // "null"')
  text=$(printf '%s' "$envelope"   | jq -r '.result // ""')
  cost=$(printf '%s' "$envelope"   | jq -r '.total_cost_usd // 0')
  class=$(classify "$is_err" "$status" "$text")
  local snippet; snippet=$(printf '%s' "$text" | head -c 200 | tr '\n' ' ')
  case "$class" in
    SUCCESS)
      log "  ok (\$$cost) — $snippet"
      # whole-line match only: mentioning the marker in prose must not stop the loop
      printf '%s\n' "$text" | grep -qxE "[[:space:]]*${RALPH_MARKER}[[:space:]]*" \
        && { log "  marker '$RALPH_MARKER' seen (own line) → COMPLETE"; return 2; }
      return 0;;
    LIMIT)     log "  USAGE/RATE LIMIT (status $status) — $snippet"; return 1;;
    TRANSIENT) log "  transient error (status $status) — $snippet"; return 4;;
    FATAL)     log "  FATAL (status $status) — $snippet"; return 3;;
  esac
}

log "=== ralph start (model=$RALPH_MODEL fallback=${RALPH_FALLBACK_MODEL:-none} marker=$RALPH_MARKER max=$RALPH_MAX_ITER yolo=$RALPH_YOLO) ==="
git_baseline
iter=$(cat "$ITER_FILE")
lwait=0; twait=0
while :; do
  [[ -f "$STOP_FILE" ]] && { log "STOP file present → halting"; rm -f "$STOP_FILE"; break; }
  if [[ "$RALPH_MAX_ITER" -gt 0 && "$iter" -ge "$RALPH_MAX_ITER" ]]; then
    log "max iterations ($RALPH_MAX_ITER) reached → halting"; break; fi
  next=$((iter + 1))
  run_one "$next"; rc=$?
  case "$rc" in
    0) iter=$next; echo "$iter" > "$ITER_FILE"; lwait=0; twait=0; git_dirty_warn
       [[ "$ONCE" == "1" ]] && { log "--once → stop"; break; };;
    2) iter=$next; echo "$iter" > "$ITER_FILE"; git_dirty_warn; log "=== ralph COMPLETE after $iter iterations ==="; break;;
    1) # usage/rate limit — wait it out, unlimited retries, capped exponential backoff
       lwait=$(( lwait == 0 ? RALPH_LIMIT_WAIT : lwait * 2 ))
       (( lwait > RALPH_LIMIT_WAIT_MAX )) && lwait=$RALPH_LIMIT_WAIT_MAX
       log "  limit backoff: sleeping ${lwait}s, then retry iter $next"; sleep "$lwait";;
    4) # transient — short capped backoff, unlimited retries
       twait=$(( twait == 0 ? RALPH_TRANSIENT_WAIT : twait * 2 ))
       (( twait > RALPH_TRANSIENT_WAIT_MAX )) && twait=$RALPH_TRANSIENT_WAIT_MAX
       log "  transient backoff: sleeping ${twait}s, then retry iter $next"; sleep "$twait";;
    3) log "=== ralph ABORTED (fatal — check config/auth/model) ==="; exit 1;;
  esac
done
