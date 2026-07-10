# Suitcase bootstrap — sourced first by generated ~/.bashrc, ~/.zshrc
#
# Defines sc_source: source a file if it's readable, otherwise print a clear,
# actionable warning instead of letting the shell spit out a cryptic
# "no such file or directory" on every startup.

sc_source() {
  if [ -r "$1" ]; then
    . "$1"
  else
    printf '\033[33msuitcase: missing %s — run %s/install.sh\033[0m\n' \
      "$1" "${SUITCASE:-<SUITCASE unset>}" >&2
  fi
}
