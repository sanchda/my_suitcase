# Atuin shell initialization — shared by bash/zsh suitcase rc files.
# Skips silently if atuin isn't installed yet.
#
# MUST be sourced at TOP-LEVEL (global) scope, never from inside a shell
# function. bash-preexec runs `declare -a preexec_functions` at its top level;
# under a function frame that array becomes function-local, so atuin's hook is
# lost when the frame returns and nothing gets recorded. The rc file sources
# this directly (not via sc_source) for exactly this reason.

command -v atuin >/dev/null 2>&1 || return 0

if [ -n "${BASH_VERSION-}" ]; then
  # bash-preexec is a hard precondition: atuin's bash integration registers
  # __atuin_preexec / __atuin_precmd into preexec_functions / precmd_functions,
  # but those arrays only fire if bash-preexec's DEBUG trap is installed.
  # Without it, atuin records nothing and Up-arrow / Ctrl-R show empty
  # results in every new shell. Install via personalize/scripts/get_bash-preexec.sh.
  [ -r "$HOME/.bash-preexec.sh" ] || return 0
  . "$HOME/.bash-preexec.sh"
  eval "$(atuin init bash)"
elif [ -n "${ZSH_VERSION-}" ]; then
  # zsh has builtin precmd/preexec — no helper needed.
  eval "$(atuin init zsh)"
fi
