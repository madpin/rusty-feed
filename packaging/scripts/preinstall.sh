#!/bin/sh
set -eu

if ! getent group rustss >/dev/null 2>&1; then
  groupadd --system rustss
fi
if ! getent passwd rustss >/dev/null 2>&1; then
  useradd --system --gid rustss --home-dir /var/lib/rustss --create-home --shell /usr/sbin/nologin rustss
fi
install -d -m 0750 -o rustss -g rustss /etc/rustss /var/lib/rustss
