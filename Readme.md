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
bin/                    On PATH: sc-doctor, getCost.sh
atuin/  tmux.conf  htoprc
personalize/  scripts/  Optional per-machine extras (run manually)
```

Generated rc files source `shell/boot.sh` first, then use `sc_source` for
everything else — so a moved or renamed file prints a clear, non-fatal warning
instead of a cryptic error on every login.

## Restore

Replaced files are backed up under `~/dotbak/SCB_<random>/`. To roll back, move
the originals out of there and delete the suitcase-generated ones (the ones
whose first line is `#DAVEGEN_SC`).
