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
if command -v pyenv 1>/dev/null 2>&1; then
  eval "$(pyenv init -)"
fi

# Mac overrides.  We don't check that things are installed, since that was
# checked by the installer
case $(uname -s) in
  "Darwin")
    LC_CTYPE=C
    PATH="/usr/local/opt/coreutils/libexec/gnubin:$PATH"  # Yum, hardcoded!
    PATH="/usr/local/opt/grep/libexec/gnubin:$PATH"
  ;;
esac

# don't put duplicate lines in the history. See bash(1) for more options
# ... or force ignoredups and ignorespace
export HISTSIZE=1000
export HISTFILESIZE=2000
export HISTIGNORE=$'[ \t]*:&:[fb]g:exit:ls'
export HISTCONTROL=ignoredups:ignorespace:erasedups  # Avoid duplicates


if [ -n "$ZSH_VERSION" ]; then
  HISTFILE=~/.zsh_history
  SAVEHIST=5000
  export SAVEHIST
  HISTDUP=erase
  export HISTDUP
  setopt appendhistory
  setopt sharehistory
  setopt incappendhistory
  SHELL=$(which zsh)
elif [ -n "$BASH_VERSION" ]; then
  shopt -s histappend     # Append, don't overwrite, history
  shopt -s checkwinsize   # Check window size after each command
  SHELL=$(which bash)
fi

# If using rlwrap, might as well us the environment vars
RLWRAP_EDITOR=("$(which vim)" '+call cursor(%L,%C)')
SVN_EDITOR="$(which vim)"
GIT_EDITOR="$(which vim)"
GIT_PAGER=("$(which vim)" - -R -c 'set foldmethod=syntax')
EDITOR="$(which vim)"
export RLWRAP_EDITOR
export SVN_EDITOR
export GIT_EDITOR
export GIT_PAGER
export EDITOR

# Grab my htop configs too!
SC_HTOPRC=${SUITCASE}/htoprc
export SC_HTOPRC

[ -x /usr/bin/lesspipe ] && eval "$(SHELL=/bin/sh lesspipe)"   # Nicer less for non-text files
if [ -z "$debian_chroot" ] && [ -r /etc/debian_chroot ]; then  # Identify chroot, if any
    debian_chroot=$(cat /etc/debian_chroot)
fi

case "$TERM" in  # set a fancy prompt (non-color, unless we know otherwise)
    xterm-color) color_prompt=yes;;
esac

force_color_prompt=yes  # Force colors?
if [ -n "$force_color_prompt" ]; then
    color_prompt=$([ -x /usr/bin/tput ] && tput setaf 1>&/dev/null && echo "yes" || echo "")
fi


if [ -n "$ZSH_VERSION" ]; then
    eval "$(starship init zsh)"
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
if [ -f "$SUITCASE/bash_scripts" ];     then source "$SUITCASE/bash_scripts"; fi           # Install AWS functions
if [ -f "$SUITCASE/bash_completion" ];  then source "$SUITCASE/bash_completion"; fi        # Install David's completion
if [[ "$(uname -v)" = *"Micro"* ]];     then source "$SUITCASE/wsl_scripts.sh"; fi         # WSL scripts
if [ -f /etc/bash_completion ] && ! shopt -oq posix; then source /etc/bash_completion; fi  # Handy completion!
#if [ -f "$SUITCASE/aws_scripts" ];    then source "$SUITCASE/aws_scripts"; fi            # AWS stuff

## Finalize
export DAVE_LOADED=1

