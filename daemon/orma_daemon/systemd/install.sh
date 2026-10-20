#!/bin/sh
# Installs ormad as a system service from a shell that sourced the workspace. The binary is
# copied: the service user cannot enter a workspace under a home directory.
set -eu
here=$(dirname "$(readlink -f "$0")")
ormad=$(command -v ormad)
sudo install -m 755 "$ormad" /usr/local/sbin/ormad
sudo install -D -m 644 "$here/orma.sysusers" /etc/sysusers.d/orma.conf
sudo systemd-sysusers /etc/sysusers.d/orma.conf
sudo install -m 644 "$here/ormad.service" /etc/systemd/system/ormad.service
sudo systemctl daemon-reload
systemd-analyze security ormad.service | tail -n 1
sudo usermod -aG orma "$USER"
echo "installed; start with: sudo systemctl enable --now ormad, or restart it after an update"
echo "$USER joins the group orma at the next login"
