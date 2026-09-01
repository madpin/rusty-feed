#!/bin/sh
set -eu

if ! getent group rusty-feed >/dev/null 2>&1; then
  groupadd --system rusty-feed
fi
if ! getent passwd rusty-feed >/dev/null 2>&1; then
  useradd --system --gid rusty-feed --home-dir /var/lib/rusty-feed --create-home --shell /usr/sbin/nologin rusty-feed
fi
install -d -m 0750 -o rusty-feed -g rusty-feed /etc/rusty-feed /var/lib/rusty-feed
