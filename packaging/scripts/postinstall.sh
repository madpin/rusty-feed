#!/bin/sh
set -eu

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload || true
fi
printf '%s\n' 'RustSS installed. Copy /usr/share/doc/rustss/rustss.env.example to /etc/rustss/rustss.env, set secrets and DATABASE_URL, then enable rustss.service.'
