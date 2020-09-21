[[ "$-" != *i* ]]  && return # Don't do anything if not interactive

# Be able to reference myself
export GPG_TTY=$(tty)
export PATH=${SUITCASE}/bin:$PATH

# Work-specific overrides
if [ -f ${HOME}/.workstuff/workstuff ]; then
  . ~/.workstuff/workstuff
fi

# pyenv overrides
if command -v pyenv 1>/dev/null 2>&1; then
  eval "$(pyenv init -)"
fi

# Mac overrides.  We don't check that things are installed, since that was
# checked by the installer
if [ "Darwin" == $(uname -s) ]; then
  LC_CTYPE=C
  PATH="/usr/local/opt/coreutils/libexec/gnubin:$PATH"  # Yum, hardcoded!
  PATH="/usr/local/opt/grep/libexec/gnubin:$PATH"
fi

# don't put duplicate lines in the history. See bash(1) for more options
# ... or force ignoredups and ignorespace
export HISTSIZE=1000
export HISTFILESIZE=2000
export HISTIGNORE=$'[ \t]*:&:[fb]g:exit:ls'
export HISTCONTROL=ignoredups:ignorespace:erasedups  # Avoid duplicates
shopt -s histappend     # Append, don't overwrite, history
shopt -s checkwinsize   # Check window size after each command

# If using rlwrap, might as well us the environment vars
export RLWRAP_EDITOR="$(which vim) '+call cursor(%L,%C)'"
export SVN_EDITOR="$(which vim)"
#export GIT_EDITOR="$(which vim)"
#export GIT_PAGER="$(which vim)"" - -R -c 'set foldmethod=syntax"
export EDITOR="$(which vim)"

# Grab my htop configs too!
export SC_HTOPRC=${SUITCASE}/htoprc

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

if [ "$color_prompt" = yes ]; then
    PS1='${debian_chroot:+($debian_chroot)}\[\033[01;32m\]\u@\h\[\033[00m\]:\[\033[01;34m\]\w\[\033[00m\]\$ '
else
    PS1='${debian_chroot:+($debian_chroot)}\u@\h:\w\$ '
fi
unset color_prompt force_color_prompt

# If this is an xterm set the title to user@host:dir
case "$TERM" in
xterm*|rxvt*)
    PS1="\[\e]0;${debian_chroot:+($debian_chroot)}\u@\h: \w\a\]$PS1"
    ;;
*)
    ;;
esac

if [ -x /usr/bin/dircolors ]; then  # If we have dircolors, use dircolors
    test -r ~/.dircolors && eval "$(dircolors -b ~/.dircolors)" || eval "$(dircolors -b)"
    alias ls='ls --color=auto'
    alias grep='grep --color=auto'
    alias fgrep='fgrep --color=auto'
    alias egrep='egrep --color=auto'
fi

# includes
if [ -f ${SUITCASE}/bash_aliases ];     then . ${SUITCASE}/bash_aliases; fi           # Use suitcase aliases
if [ -f ${SUITCASE}/bash_scripts ];     then . ${SUITCASE}/bash_scripts; fi           # Install AWS functions
if [ -f ${SUITCASE}/aws_scripts ];      then . ${SUITCASE}/aws_scripts; fi            # AWS stuff
if [[ "$(uname -v)" = *"Micro"* ]];     then . ${SUITCASE}/wsl_scripts.sh; fi         # WSL scripts
if [ -f /etc/bash_completion ] && ! shopt -oq posix; then . /etc/bash_completion; fi  # Handy completion!
if [ -f ${SUITCASE}/bash_completion ];  then . ${SUITCASE}/bash_completion;fi         # Install David's completion

## Finalize
export DAVE_LOADED=1
