#!/bin/sh
# Fail when a Linux release binary needs a newer glibc than we support.
#
# GitHub's `ubuntu-latest` moved to Ubuntu 26.04 (glibc 2.43); binaries linked
# there require GLIBC_2.43 and do not start on Debian 13 (2.41, our runtime
# image) or Ubuntu 24.04 (2.39). See docs/FINDINGS.md FND-250.
#
# Usage: scripts/check-glibc-floor.sh <dir-with-binaries> [max-glibc]
set -eu

dir=${1:?usage: check-glibc-floor.sh <dir> [max-glibc]}
max=${2:-2.39}
status=0

for bin in rusty-feed-server rusty-feed-worker rusty-feed-cli; do
  path="$dir/$bin"
  [ -f "$path" ] || { printf 'missing binary: %s\n' "$path" >&2; exit 2; }
  needed=$(objdump -T "$path" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n 1)
  [ -n "$needed" ] || { printf '%s: no versioned glibc symbols found\n' "$bin" >&2; exit 2; }
  highest=$(printf '%s\n%s\n' "$needed" "$max" | sort -V | tail -n 1)
  if [ "$highest" != "$max" ]; then
    printf '%s needs glibc %s; the supported maximum is %s\n' "$bin" "$needed" "$max" >&2
    status=1
  else
    printf '%s needs glibc %s (max %s): ok\n' "$bin" "$needed" "$max"
  fi
done

exit "$status"
