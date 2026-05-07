# Atuin shell initialization — shared by bash/zsh suitcase rc files.
# Skips silently if atuin isn't installed yet.

command -v atuin >/dev/null 2>&1 || return 0

if [ -n "${BASH_VERSION-}" ]; then
  eval "$(atuin init bash)"
elif [ -n "${ZSH_VERSION-}" ]; then
  eval "$(atuin init zsh)"
fi
