# shellcheck shell=bash
if ! command -v compdef > /dev/null 2>&1; then
  autoload -U +X compinit && compinit
fi
if command -v register-python-argcomplete3 > /dev/null 2>&1; then
  eval "$(register-python-argcomplete3 --shell zsh armo)"
elif command -v register-python-argcomplete > /dev/null 2>&1; then
  eval "$(register-python-argcomplete --shell zsh armo)"
fi
