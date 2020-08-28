# Opens Dave's suitcase
header_sc="#DAVEGEN_SC"
cfg_bash=${HOME}/.bashrc
if [ "Darwin" == $(uname -s) ]; then
  # We need to do some things, since I wrote this for Linux
  if ! type "greadlink" > /dev/null; then
    echo "You're on Mac, but you haven't installed coreutils.  I sort of need that."
    exit -1
  fi
  LC_CTYPE=C
  PATH="/usr/local/opt/coreutils/libexec/gnubin:$PATH"
  cfg_bash=${HOME}/.bash_profile
fi
bakdir=${HOME}/dotbak/"SCB_"$(cat /dev/urandom | tr -cd 'a-f0-9' | head -c 16)

# Figure out where the install.sh script lives.  That's going to be the root of
# the suitcase
SUITCASE=$(dirname "$(readlink -f "$0")")

# bashrc
if [ -f ${SUITCASE}/bashrc ]; then
  # If we're dealing with a suitcase-generated ~/.bashrc, then just destroy it.
  # Else, attempt to preserve it
  if [ -f ${cfg_bash} ]; then
    if [[ $(head -n 1 ${cfg_bash}) = "${header_sc}" ]]; then
      echo "Suitcase bashrc detected.  Deleting."
      rm -rf ${cfg_bash}
    else
      echo "Non-suitcase bashrc detected.  Backing to" ${bakdir}
      mkdir -p ${bakdir}   # Make the backup directory
      mv ${cfg_bash} ${bakdir}
    fi
  fi

  # Create the new bashrc
  echo "#DAVEGEN_SC"                 >  ${cfg_bash}
  echo "export SUITCASE=${SUITCASE}" >> ${cfg_bash}
  echo ". ${SUITCASE}/bashrc"        >> ${cfg_bash}
  . ${cfg_bash}
fi

# vimrc 
if [ -f ${SUITCASE}/vimrc ] && [ -d ${SUITCASE}/vim ]; then
  # Detect whether this is a David-generated vimrc
  if [ -f ${HOME}/.vimrc ]; then
    if [ $(head -n 1 ${HOME}/.vimrc) = "\"${header_sc}" ]; then
      echo "Suitcase vimrc detected.  Deleting."
      rm -rf ${HOME}/.vimrc
    else
      echo "Non-suitcase vimrc detected.  Backing to" ${bakdir}
      mkdir -p ${bakdir}   # Make the backup directory
      mv ${HOME}/.vimrc ${bakdir}
    fi
  else
    echo "No vimrc detected, installing suitcase version" 
  fi

  # Create new ~/.vimrc
  echo "\"${header_sc}" > ${HOME}/.vimrc
  echo "set nocompatible" >> ${HOME}/.vimrc

  # Set runtimepath
  rtp="set runtimepath="
  rtp+="${SUITCASE}/vim,"
  rtp+="/var/lib/vim/addons,"
  rtp+="/usr/share/vim/vimfiles,"
  for i in $(ls /usr/share/vim | grep "^vim[0-9]"); do
    rtp+="/usr/share/vim/"$i","
  done
  rtp+="/usr/share/vim/vimfiles/after,"
  rtp+="/var/lib/vim/addons/after,"
  rtp+="${SUITCASE}/vim/after"
  echo $rtp >> ${HOME}/.vimrc

  # Finally, import the real vimrc
  echo "source $SUITCASE/vimrc" >> ${HOME}/.vimrc
fi

# tmux.conf
if [ -f ${SUITCASE}/tmux.conf ]; then

  # Detect whether this is a David-generated tmux.conf
  if [ -f ${HOME}/.tmux.conf ]; then
    if [ "$(head -n 1 ${HOME}/.tmux.conf)" = "${header_sc}" ]; then
      echo "Suitcase tmux.conf detected.  Deleting."
      rm -rf ${HOME}/.tmux.conf
    else
      echo "Non-suitcase tmux.conf detected.  Backing to" ${bakdir}
      mkdir -p ${bakdir}   # Make the backup directory
      mv ${HOME}/.tmux.conf ${bakdir}
    fi
  else
    echo "No tmux.conf detected, installing suitcase version" 
  fi

  # Very slim
  echo "${header_sc}" > ${HOME}/.tmux.conf
  echo "source-file ${SUITCASE}/tmux.conf" >> ${HOME}/.tmux.conf
fi

# rlwrap histories
[ ! -d ${HOME}/history ] && mkdir -m +6000 ${HOME}/history

# Subversion
if [ -d ${SUITCASE}/subversion ]; then

  # Detect whether we have installed our version of subversion
  if [ -d ${HOME}/.subversion ]; then
    if [ -f ${HOME}/.subversion/DAVEGEN_SC ]; then
      echo "Suitcase subversion detected.  Deleting."
      rm -rf ${HOME}/.subversion/{config,servers}
    else
      echo "Non-suitcase subversion config detected.  Backing to" ${bakdir}
      mkdir -p ${bakdir}/subversion
      mv ${HOME}/.subversion/{config,servers} ${bakdir}/subversion
    fi
  fi

  # Put the suitcase file in place
  mkdir -p ${HOME}/.subversion
  touch ${HOME}/.subversion/DAVEGEN_SC

  # Create symlinks (TODO: does subversion have import or source???)
  ln -s ${SUITCASE}/subversion/config  ${HOME}/.subversion/config
  ln -s ${SUITCASE}/subversion/servers ${HOME}/.subversion/servers
fi
