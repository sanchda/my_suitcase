Dave's Suitcase
===

I code on the reference system.

But how do you get there?

I pack a suitcase.

---

## Install

```sh
git clone <this-repo> ~/suitcase   # clone wherever you like
cd ~/suitcase
./install.sh
```

The installer resolves `$SUITCASE` to wherever you cloned the repo (it derives
the path from its own location), so the clone directory above is just an
example — any path works. The installer is modular and safe to re-run. It generates `~/.bashrc`,
`~/.zshrc`, and `~/.zshenv` (each marked with a `#DAVEGEN_SC` header), installs
`tmux.conf`, and links the atuin config. Any pre-existing, non-suitcase file it
would overwrite is moved to `~/dotbak/SCB_<random>/` first.

Open a new shell afterward to pick up the config.

## Verify

```sh
sc-doctor
```

`sc-doctor` (on your `PATH` via the suitcase `bin/`) checks that `SUITCASE`
resolves, that the generated rc files are suitcase-owned, that every file they
source exists, and that expected tools are present. It prints `✓ / ⚠ / ✗` and
exits non-zero if anything is broken. `install.sh` runs it automatically at the
end.

## Layout

```
install.sh              Orchestrator; runs each install/ module, then sc-doctor
install/                Install modules (shell, tmux, atuin, macos) + common.sh
shell/
  boot.sh               Defines sc_source (guarded sourcing helper)
  core.sh               Shared bash+zsh: PATH, aliases, history, editor, …
  bash.sh               Bash-only: prompt, shopt, completions
  zsh.sh                Zsh-only: setopt, starship, atuin
  functions/*.sh        Auto-sourced shell functions (git worktree helpers, …)
bin/                    On PATH: sc-doctor, cc-mod, cc-statusline, getCost.sh
claude/mods/            Reversible ~/.claude/settings.json fragments (cc-mod)
lib/cc-mod.jq           The merge/unmerge jq library behind cc-mod
tools/                  Rust tools: bark, ralph, ralphd, claude-top
atuin/  tmux.conf  htoprc
personalize/  scripts/  Optional per-machine extras (run manually)
tests/                  Fixture tests for the shell tooling (tests/cc-mod.sh)
```

## Claude Code config

`~/.claude/settings.json` is managed in reversible pieces called mods: a status
line, an ExitPlanMode plan reviewer, Discord notifications. `cc-mod list` shows
them, `cc-mod disable <mod>` takes one back out (and it stays out across
personalize runs). Each has a `personalize/scripts/` switch taking the same
flags -- `setup_claude_statusline.sh --disable`, `setup_bark.sh --status`. See
`claude/mods/README.md`.

Generated rc files source `shell/boot.sh` first, then use `sc_source` for
everything else — so a moved or renamed file prints a clear, non-fatal warning
instead of a cryptic error on every login.

## Restore

Replaced files are backed up under `~/dotbak/SCB_<random>/`. To roll back, move
the originals out of there and delete the suitcase-generated ones (the ones
whose first line is `#DAVEGEN_SC`).
