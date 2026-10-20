if type register-python-argcomplete3 > /dev/null 2>&1; then
  eval "$(register-python-argcomplete3 orma)"
elif type register-python-argcomplete > /dev/null 2>&1; then
  eval "$(register-python-argcomplete orma)"
fi
