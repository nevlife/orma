#!/bin/sh
# Installs armod as a system service from a shell that sourced the workspace. The binary is
# copied: the service user cannot enter a workspace under a home directory.
set -eu
here=$(dirname "$(readlink -f "$0")")
armod=$(command -v armod)
sudo install -m 755 "$armod" /usr/local/sbin/armod
sudo install -D -m 644 "$here/armo.sysusers" /etc/sysusers.d/armo.conf
sudo systemd-sysusers /etc/sysusers.d/armo.conf
sudo install -m 644 "$here/armod.service" /etc/systemd/system/armod.service
sudo systemctl daemon-reload
systemd-analyze security armod.service | tail -n 1
sudo usermod -aG armo "$USER"
echo "installed; start with: sudo systemctl enable --now armod, or restart it after an update"
echo "$USER joins the group armo at the next login"
