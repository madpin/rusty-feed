#!/usr/bin/env sh
set -eu

# Container entrypoint for RustSS.
#
# From M2 onward, apply database migrations before starting the server when
# AUTO_MIGRATE is enabled (default). `migrate` is a stub until M2, so it is
# gated behind the flag to keep the M0/M1 image runnable.
if [ "${AUTO_MIGRATE:-false}" = "true" ]; then
    echo "[entrypoint] applying database migrations..."
    rustss-cli migrate
fi

echo "[entrypoint] starting: $*"
exec "$@"
