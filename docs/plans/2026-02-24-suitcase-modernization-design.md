# Suitcase Modernization Design

## Goal

Streamline, modernize, and compartmentalize the dotfiles suitcase. Make the
installer modular, shell configs friendly to both bash and zsh, and remove
unused components (vim, subversion).

## Directory Structure

```
my_suitcase/
├── install.sh                  # Orchestrator (~20 lines)
├── install/
│   ├── common.sh               # Backup helper, OS detection, symlink util
│   ├── shell.sh                # Install bashrc/zshrc pointing to shell/
│   ├── tmux.sh                 # Install tmux.conf
│   └── macos.sh                # Brew packages, GNU path setup
├── shell/
│   ├── core.sh                 # Shared: env, PATH, aliases, editor, history
│   ├── bash.sh                 # shopt, PROMPT_COMMAND, PS1, bash completions
│   ├── zsh.sh                  # setopt, starship, zsh completions
│   └── functions/
│       └── git.sh              # gwA, gwD, gwCheck, gwPrune (unchanged)
├── tmux.conf
├── htoprc
├── bin/
├── personalize/
└── scripts/
```

## Removals

- `vim/` directory (entire submodule tree)
- `vimrc` file
- `subversion/` directory
- `.gitmodules` entries for vim submodules
- All vim/vimrc/neovim installer logic
- All subversion installer logic
- Commented-out pyenv/rbenv blocks
- `bash_aliases` (merged into shell/core.sh)
- `bash_scripts` (moved to shell/functions/git.sh)
- `bash_completion` (empty, deleted)
- `bashrc` (replaced by shell/core.sh + shell/bash.sh)

## Installer Design

### install.sh

Thin orchestrator. Sources `install/common.sh`, then calls each module in
order. On macOS, also calls `install/macos.sh`. Each module can also be run
standalone.

### install/common.sh

Shared utilities:

- `SUITCASE` — resolved path to the repo root
- `HEADER="#DAVEGEN_SC"` — sentinel for suitcase-generated files
- `backup_if_needed <file>` — if file has the header, delete it; otherwise
  back up to `~/dotbak/SCB_<random>/`
- `detect_os` — sets `$SC_OS` to `darwin`, `linux`, or `wsl`
  (WSL detected via `/proc/version` containing "microsoft")

### install/shell.sh

- Backs up existing `~/.bashrc` (and `~/.bash_profile` on Mac)
- Writes thin `~/.bashrc`: exports `SUITCASE`, sources `shell/core.sh` +
  `shell/bash.sh`
- Writes thin `~/.zshrc`: exports `SUITCASE`, sources `shell/core.sh` +
  `shell/zsh.sh`

### install/tmux.sh

Same backup-then-write pattern using shared helpers from common.sh.

### install/macos.sh

- Checks for Homebrew, installs if missing
- `brew install coreutils findutils gnu-tar gnu-sed gawk grep`
- No pyenv, llvm, or spaceship

## Shell Config Design

### shell/core.sh (sourced by both bash and zsh)

- Interactive guard: `[[ "$-" != *i* ]] && return`
- GPG_TTY, PATH (suitcase bin)
- Locale: LC_ALL=C, LC_LANG=C
- Work-specific overrides (~/.workstuff/workstuff)
- macOS GNU path fixups (Darwin case block)
- History: HISTSIZE, HISTFILESIZE, HISTCONTROL, HISTIGNORE, HISTFILE
- Editors: EDITOR, SVN_EDITOR, GIT_EDITOR, GIT_PAGER, RLWRAP_EDITOR
- htoprc export
- lesspipe, dircolors + color aliases
- All aliases (ll, rm, cp, du, df, less, path, openports, wget, grep,
  tmuxs, mytop, k, gw*)
- Source all files in shell/functions/*.sh

### shell/bash.sh (bash-only)

- HISTTIMEFORMAT
- shopt -s histappend cmdhist checkwinsize
- PROMPT_COMMAND="history -a; $PROMPT_COMMAND"
- export SHELL=$(which bash)
- Debian chroot detection
- PS1 (color prompt logic, xterm title)

### shell/zsh.sh (zsh-only)

- SAVEHIST, setopt history options
- export SHELL=$(which zsh)
- Starship init (if available)

### shell/functions/git.sh

Moved from bash_scripts, unchanged: gwA, gwD, _gwStaleWorktrees, gwCheck,
gwPrune.

## Neovim

Not managed by this repo. Neovim config lives in its own repo at
~/.config/nvim. The suitcase does not touch it.

## Unchanged

- bin/ — no changes
- personalize/ — no changes
- scripts/ — no changes
- tmux.conf — content unchanged, installer modernized
- htoprc — no changes
