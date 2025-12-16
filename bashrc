[[ "$-" != *i* ]]  && return # Don't do anything if not interactive

# Be able to reference myself
GPG_TTY=$(tty)
export GPG_TTY
PATH=${SUITCASE}/bin:$PATH
export PATH

# Work-specific overrides
if [ -f "$HOME"/.workstuff/workstuff ]; then
  source "$HOME"/.workstuff/workstuff
fi

# pyenv overrides
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

# rbenv overrides
if [ -d "$HOME/.rbenv" ]; then
  if [[ ":$PATH:" != *":$HOME/.rbenv/bin:"* ]]; then
    export PATH="$HOME/.rbenv/bin:$PATH"
  fi
  if command -v rbenv 1>/dev/null 2>&1; then
    eval "$(rbenv init -)"
  fi
fi

# nvm overrides
if [ -d "$HOME/.nvm" ]; then
  export NVM_DIR="$HOME/.nvm"
  [ -s "$NVM_DIR/nvm.sh" ] && source "$NVM_DIR/nvm.sh"
  [ -s "$NVM_DIR/bash_completion" ] && source "$NVM_DIR/bash_completion"
fi

# Mac overrides.  We don't check that things are installed, since that was
# checked by the installer
case "$(uname -s)" in
  "Darwin")
    export LC_CTYPE=C
    for dir in /usr/local/opt/coreutils/libexec/gnubin /usr/local/opt/grep/libexec/gnubin /usr/local/opt/llvm/bin; do
      if [[ ":$PATH:" != *":$dir:"* ]]; then
        export PATH="$dir:$PATH"
      fi
    done
  ;;
esac

# Common history settings
export HISTSIZE=10000
export HISTFILESIZE=2000000
export HISTCONTROL=ignoreboth:erasedups
export HISTIGNORE='ls:ll:ls -alh:pwd:clear:history'
export HISTFILE=~/.shared_history  # Use a common history file for Bash and Zsh

# Bash-specific settings
if [ -n "$BASH_VERSION" ]; then
  export HISTTIMEFORMAT='%F %T '
  shopt -s histappend cmdhist checkwinsize
  PROMPT_COMMAND="history -a; history -c; history -r; $PROMPT_COMMAND"
  export SHELL=$(which bash)
elif [ -n "$ZSH_VERSION" ]; then
  export SAVEHIST=10000
  setopt SHARE_HISTORY APPEND_HISTORY INC_APPEND_HISTORY
  setopt HIST_EXPIRE_DUPS_FIRST
  setopt HIST_IGNORE_DUPS
  setopt HIST_IGNORE_ALL_DUPS
  setopt HIST_FIND_NO_DUPS
  setopt HIST_IGNORE_SPACE
  setopt HIST_SAVE_NO_DUPS
  export SHELL=$(which zsh)
fi

# Editor settings
VIM_PATH="$(which vim)"
export EDITOR="$VIM_PATH"
export SVN_EDITOR="$VIM_PATH"
export GIT_EDITOR="$VIM_PATH"
export GIT_PAGER=("$VIM_PATH" - -R -c 'set foldmethod=syntax')
export RLWRAP_EDITOR=("$VIM_PATH" '+call cursor(%L,%C)')

# Grab my htop configs too!
export SC_HTOPRC="${SUITCASE}/htoprc"

# Nicer less for non-text files
[ -x /usr/bin/lesspipe ] && eval "$(SHELL=/bin/sh lesspipe)"

# Identify chroot, if any
if [ -z "$debian_chroot" ] && [ -r /etc/debian_chroot ]; then
    debian_chroot=$(cat /etc/debian_chroot)
fi

# set a fancy prompt (non-color, unless we know otherwise)
case "$TERM" in
    xterm-color|*-256color) color_prompt=yes;;  # Add support for 256 color terminals
esac

# Force colors?
force_color_prompt=yes
if [ "$force_color_prompt" = yes ]; then
    if [ -x /usr/bin/tput ] && tput setaf 1 &>/dev/null; then
        color_prompt=yes
    else
        color_prompt=no
    fi
fi

# Set the prompt
if [ -n "$ZSH_VERSION" ]; then
    if command -v starship >/dev/null 2>&1; then
        eval "$(starship init zsh)"
    fi
elif [ "$color_prompt" = yes ]; then
    PS1='${debian_chroot:+($debian_chroot)}\[\033[01;32m\]\u@\h\[\033[00m\]:\[\033[01;34m\]\w\[\033[00m\]\$ '
else
    PS1='${debian_chroot:+($debian_chroot)}\u@\h:\w\$ '
fi
unset color_prompt force_color_prompt

# If this is an xterm set the title to user@host:dir
if [ -z "$ZSH_VERSION" ]; then
  case "$TERM" in
  xterm*|rxvt*)
      PS1="\[\e]0;${debian_chroot:+($debian_chroot)}\u@\h: \w\a\]$PS1"
      ;;
  *)
      ;;
  esac
fi

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

# includes
if [ -f "$SUITCASE/bash_aliases" ];     then source "$SUITCASE/bash_aliases"; fi           # Use suitcase aliases
#if [ -f "$SUITCASE/bash_scripts" ];     then source "$SUITCASE/bash_scripts"; fi           # Install AWS functions
#if [ -f "$SUITCASE/bash_completion" ];  then source "$SUITCASE/bash_completion"; fi        # Install David's completion
#if [ -f /etc/bash_completion ] && ! shopt -oq posix; then source /etc/bash_completion; fi  # Handy completion!
if [ -f "$SUITCASE/overrides" ];     then source "$SUITCASE/overrides"; fi

## Finalize
export DAVE_LOADED=1
#export JAVA_HOME="/usr/lib/jvm/java-17-openjdk-amd64"
#export PATH=$JAVA_HOME:$PATH
