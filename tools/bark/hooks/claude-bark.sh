#!/bin/bash
# Claude Code hook -> bark. Reads the hook JSON on stdin and posts one line, so
# you hear about a session that finished or wants input without watching it.
#
# Wired up by the bark-notify cc-mod (cc-mod enable bark-notify, or
# personalize/scripts/setup_bark.sh). Always exits 0: a notifier must never break
# a session.
#
# Env knobs:
#   BARK_CC_EVENTS  comma-separated events to post (default Notification,Stop,
#                   SessionEnd; SubagentStop is also understood)
#   BARK_CC_TO      bark target name; otherwise the config default
#   BARK_BIN        path to bark; otherwise PATH, then ~/.local/bin/bark
set -u

BARK="${BARK_BIN:-}"
if [ -z "$BARK" ]; then
  BARK="$(command -v bark 2>/dev/null || echo "$HOME/.local/bin/bark")"
fi
[ -x "$BARK" ] || exit 0
command -v jq >/dev/null 2>&1 || exit 0

payload="$(cat)"
field() { printf '%s' "$payload" | jq -r "$1 // empty" 2>/dev/null; }

event="$(field .hook_event_name)"
events="${BARK_CC_EVENTS:-Notification,Stop,SessionEnd}"
case ",$events," in
  *",$event,"*) ;;
  *) exit 0 ;;
esac

# A Stop hook that already forced a continuation would just double-post.
[ "$(field .stop_hook_active)" = "true" ] && exit 0

cwd="$(field .cwd)"
[ -n "$cwd" ] || cwd="$PWD"
session="$(field .session_id)"
transcript="$(field .transcript_path)"

# Last thing Claude actually said, so the ping carries the outcome. Tail-limited
# because transcripts get large; fromjson? drops the line tail may have cut.
last_said() {
  [ -f "$transcript" ] || return 0
  tail -n 200 "$transcript" 2>/dev/null | jq -Rrs '
    split("\n") | map(fromjson? // empty)
    | map(select(.type == "assistant")
          | (.message.content // [])
          | map(select(.type == "text") | .text)
          | join(" "))
    | map(select(length > 0)) | last // ""' 2>/dev/null |
    tr '\n\r\t' '   ' | cut -c1-240
}

case "$event" in
  Notification)
    text="needs you: $(field .message)"
    ;;
  Stop)
    said="$(last_said)"
    text="done${said:+: $said}"
    ;;
  SubagentStop)
    text="subagent done"
    ;;
  SessionEnd)
    reason="$(field .reason)"
    text="session ended${reason:+ ($reason)}"
    ;;
  *)
    text="$event"
    ;;
esac

id="claude/${cwd##*/}"
[ -n "$session" ] && id="$id/${session:0:4}"

args=(--timeout 5 --id "$id")
[ -n "${BARK_CC_TO:-}" ] && args+=(--to "$BARK_CC_TO")

# Hook stderr is invisible in normal use, so a rotated webhook would silently
# stop notifying. Record failures instead, newest last, tail-trimmed.
log="${BARK_CC_LOG:-$HOME/.cache/bark/claude-hook.log}"
if ! err="$("$BARK" "${args[@]}" -- "$text" 2>&1 >/dev/null)"; then
  if mkdir -p "$(dirname "$log")" 2>/dev/null; then
    printf '%s %s: %s\n' "$(date +%Y-%m-%dT%H:%M:%S%z)" "$event" \
      "${err:-bark exited nonzero}" >>"$log" 2>/dev/null
    if [ "$(wc -c <"$log" 2>/dev/null || echo 0)" -gt 65536 ]; then
      tail -n 100 "$log" >"$log.trim" 2>/dev/null && mv -f "$log.trim" "$log"
    fi
  fi
fi

exit 0
