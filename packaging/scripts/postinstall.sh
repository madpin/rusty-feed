#!/bin/sh
set -eu

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload || true
fi
printf '%s\n' 'Rusty Feed installed. Copy /usr/share/doc/rusty-feed/rusty-feed.env.example to /etc/rusty-feed/rusty-feed.env, set secrets and DATABASE_URL, then enable rusty-feed.service.'
