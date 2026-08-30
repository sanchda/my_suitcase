Claude config mods
==================

Reversible fragments of `~/.claude/settings.json`, managed by `cc-mod`. Not
Claude Code's native plugins (`/plugin`, `enabledPlugins`) -- these are local
config this repo owns.

```sh
cc-mod list                     # what exists, what's on
cc-mod enable bark-notify       # merge its fragment, write a receipt
cc-mod ensure bark-notify       # same, but skip if you disabled it or it's current
cc-mod disable bark-notify      # replay the receipt backwards
cc-mod status bark-notify       # what it owns; whether it still holds
cc-mod doctor                   # requirements, drift, stale receipts
cc-mod reapply                  # re-apply everything on (after a git pull)
cc-mod -n enable statusline     # dry run: print the settings, write nothing
```

Why receipts: enabling records the exact changes it made, so disabling puts the
file back rather than deleting whole keys. Two mods can append to `hooks.Stop`
and removing one leaves the other alone; a key you had set to something else is
restored to your value, not dropped.

`disable` is remembered. `ensure` (what `personalize` runs) skips anything you
turned off, so a personalize pass never silently re-enables your notifications.
Naming a mod in `enable` clears that.

enable vs ensure
----------------

| | `enable` | `ensure` |
|---|---|---|
| you disabled it before | turns it back on | leaves it off |
| requirements missing | fails loudly | skips, exit 0 |
| already applied and current | re-applies | does nothing, writes nothing |
| unknown mod name | fails | skips, exit 0 |

`enable` is for you at a prompt; `ensure` is for scripts.

Personalize scripts
-------------------

Each mod worth a manual switch has a script in `personalize/scripts/`, all taking
the same flags (they share `personalize/lib/cc-mod-shim.sh`):

```sh
setup_claude_statusline.sh              # ensure: on unless you disabled it
setup_claude_statusline.sh --disable    # off, and it stays off
setup_claude_statusline.sh --enable     # on again after a --disable
setup_claude_statusline.sh --status     # what it owns
```

`setup_bark.sh` and `setup_plan_vim_gate.sh` build their binary first and then
ensure their mod; `--disable` and `--status` short-circuit before the build.
`zz_setup_claude_mods.sh` is the catch-all (`ensure --default` plus `doctor`), so
a new mod with `"default": true` needs no script at all. Adding one for a new mod:

```bash
#!/bin/bash
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SUITCASE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source-path=SCRIPTDIR source=../lib/cc-mod-shim.sh
. "$SUITCASE_ROOT/personalize/lib/cc-mod-shim.sh"
cc_mod_shim <mod-name> "$@"
```

Writing one
-----------

`claude/mods/<name>/mod.json`:

```json
{
  "description": "one line, shown by cc-mod list",
  "default": true,
  "requires": {
    "commands": ["jq", "curl"],
    "files": ["{{LOCAL_BIN}}/bark"],
    "optional": ["npx"]
  },
  "settings": { "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "{{SUITCASE}}/tools/bark/hooks/claude-bark.sh" } ] } ] } },
  "links": { "agents/reviewer.md": "assets/reviewer.md" },
  "enable": "enable.sh",
  "disable": "disable.sh"
}
```

- `settings` is merged: objects recurse, **arrays append**, scalars overwrite.
- `requires` is checked before anything is written. `commands` and `files` are
  hard (enable refuses); `optional` just prints a note. A bulk
  `enable --default` skips a mod with unmet requirements instead of aborting the
  batch, so a fresh box that hasn't built a binary yet still gets everything else.
- `links` symlinks files into `~/.claude/` (relative to the settings file). An
  existing real file is moved to `<name>.pre-cc-mod` and restored on disable.
  A link is only removed while it still points at this mod.
- `enable` / `disable` scripts are optional escape hatches for anything
  declarative JSON can't express. They run with `$CC_MOD_NAME` and `$SUITCASE`.
- Placeholders in any string: `{{SUITCASE}}`, `{{MOD_DIR}}`, `{{HOME}}`,
  `{{LOCAL_BIN}}`.
- `default: true` means `enable --default` picks it up.

Adoption: enabling a mod whose config is *already* in settings.json (say you had
wired the hook by hand) takes ownership rather than duplicating it -- and then
disabling removes it.

State lives in `${XDG_STATE_HOME:-~/.local/state}/cc-mod`: `receipts/`,
`opted-out/`, and the last 10 `backups/settings.*.json` taken before each write.

Tests: `tests/cc-mod.sh` (fixture-based; never touches your real `~/.claude`).
