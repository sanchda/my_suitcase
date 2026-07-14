# Shared shell configuration — sourced by both bash and zsh
[[ "$-" != *i* ]] && return

# GPG
GPG_TTY=$(tty)
export GPG_TTY

# PATH
PATH="${SUITCASE}/bin:$PATH"
if [ -x "$HOME/.claude/local/claude" ]; then
  PATH="$HOME/.claude/local:$PATH"
fi
if [ -d "$HOME/.local/bin" ]; then
  PATH="$HOME/.local/bin:$PATH"
fi
export PATH

# Locale — C.UTF-8 keeps C's predictable sort/format behavior but enables
# UTF-8 character handling (needed for tmux to render non-ASCII glyphs).
LC_ALL=C.UTF-8
LANG=C.UTF-8
export LC_ALL LANG

# Work-specific overrides
if [ -f "$HOME/.workstuff/workstuff" ]; then
  source "$HOME/.workstuff/workstuff"
fi

# pyenv
if [ -d "$HOME/.pyenv" ]; then
  export PYENV_ROOT="$HOME/.pyenv"
  if [[ ":$PATH:" != *":$PYENV_ROOT/bin:"* ]]; then
    export PATH="$PYENV_ROOT/bin:$PATH"
  fi
  if command -v pyenv 1>/dev/null 2>&1; then
    if [ -n "$ZSH_VERSION" ]; then
      eval "$(pyenv init - zsh)"
    else
      eval "$(pyenv init - bash)"
    fi
  fi
fi

# rbenv
if [ -d "$HOME/.rbenv" ]; then
  if [[ ":$PATH:" != *":$HOME/.rbenv/bin:"* ]]; then
    export PATH="$HOME/.rbenv/bin:$PATH"
  fi
  if command -v rbenv 1>/dev/null 2>&1; then
    eval "$(rbenv init -)"
  fi
fi

# nvm
if [ -d "$HOME/.nvm" ]; then
  export NVM_DIR="$HOME/.nvm"
  [ -s "$NVM_DIR/nvm.sh" ] && source "$NVM_DIR/nvm.sh"
  [ -s "$NVM_DIR/bash_completion" ] && source "$NVM_DIR/bash_completion"
fi

# macOS GNU path fixups
case "$(uname -s)" in
  "Darwin")
    export LC_CTYPE=C.UTF-8
    for dir in /usr/local/opt/coreutils/libexec/gnubin /usr/local/opt/grep/libexec/gnubin /usr/local/opt/llvm/bin; do
      if [[ ":$PATH:" != *":$dir:"* ]]; then
        export PATH="$dir:$PATH"
      fi
    done
  ;;
esac

# History
export HISTSIZE=10000
export HISTFILESIZE=2000000
export HISTCONTROL=ignoreboth:erasedups
export HISTIGNORE='ls:ll:ls -alh:pwd:clear:history'
export HISTFILE=~/.shared_history

# Editors
VIM_PATH="$(which vim)"
export EDITOR="$VIM_PATH"
export SVN_EDITOR="$VIM_PATH"
export GIT_EDITOR="$VIM_PATH"
export GIT_PAGER=("$VIM_PATH" - -R -c 'set foldmethod=syntax')
export RLWRAP_EDITOR=("$VIM_PATH" '+call cursor(%L,%C)')

# Force IPv4 for Node (avoids slow IPv6 DNS resolution)
export NODE_OPTIONS="--dns-result-order=ipv4first"

# htoprc
export SC_HTOPRC="${SUITCASE}/htoprc"

# lesspipe
[ -x /usr/bin/lesspipe ] && eval "$(SHELL=/bin/sh lesspipe)"

# dircolors
if command -v dircolors 1>/dev/null 2>&1; then
  if test -r ~/.dircolors; then
    eval "$(dircolors -b ~/.dircolors)"
  else
    eval "$(dircolors -b)"
  fi
  alias ls='ls --color=auto'
  alias grep='grep --color=auto'
  alias fgrep='fgrep --color=auto'
  alias egrep='egrep --color=auto'
fi

# Aliases
alias ll='ls -alF'
alias rm='rm -i'
alias cp='cp -rfv'
alias du='du -h'
alias df='df -h'
alias less='less -r'
alias path='echo -e ${PATH//:/\\n}'
alias openports='netstat -tulanp'
alias wget='wget -c'
alias grep='grep --color=auto --exclude-dir="*.svn" --exclude-dir="*.git"'
alias tmuxs='tmux_sixel -L tmux_sixel'
alias mytop='HTOPRC=${SC_HTOPRC} htop'
alias k='kubectl'

# Git worktree aliases
alias gw='git worktree'
alias gwl='git worktree list'
alias gwr='git worktree remove'
alias gwa='git worktree add'
alias gwn='git fetch origin && git worktree add'
alias gwb='git worktree add -b'

# Source all function files
for _fn_file in "$SUITCASE"/shell/functions/*.sh; do
  [ -f "$_fn_file" ] && source "$_fn_file"
done
unset _fn_file

# Per-machine overrides (not tracked in git)
if [ -f "$SUITCASE/overrides" ]; then
  source "$SUITCASE/overrides"
fi
