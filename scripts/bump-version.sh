#!/bin/sh
set -eu

usage() {
  cat <<'EOF'
Usage: scripts/bump-version.sh <major|minor|patch|X.Y.Z>

Required environment:
  TEST_DATABASE_URL
  TEST_REDIS_URL
  PLAYWRIGHT_DATABASE_URL
  PLAYWRIGHT_REDIS_URL
  RUSTY_FEED_BUMP_CONFIRM_DATABASES=1

TEST_DATABASE_URL and PLAYWRIGHT_DATABASE_URL must point to different,
empty, dedicated disposable databases.
EOF
}

fail() {
  printf '%s\n' "$*" >&2
  exit 1
}

run_check() {
  printf '\n==> %s\n' "$1"
  shift
  "$@"
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
  "")
    usage >&2
    exit 2
    ;;
esac
[ "$#" -eq 1 ] || fail "expected exactly one version bump argument"

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

for command_name in cargo npm npx docker git openssl python3; do
  command -v "$command_name" >/dev/null 2>&1 || fail "required command is unavailable: $command_name"
done

: "${TEST_DATABASE_URL:?TEST_DATABASE_URL must point to a dedicated disposable integration-test database}"
: "${TEST_REDIS_URL:?TEST_REDIS_URL is required}"
: "${PLAYWRIGHT_DATABASE_URL:?PLAYWRIGHT_DATABASE_URL must point to a dedicated disposable browser-test database}"
: "${PLAYWRIGHT_REDIS_URL:?PLAYWRIGHT_REDIS_URL is required}"
[ "$TEST_DATABASE_URL" != "$PLAYWRIGHT_DATABASE_URL" ] || fail "integration and browser tests must use different databases"
[ "${RUSTY_FEED_BUMP_CONFIRM_DATABASES:-}" = "1" ] || fail "set RUSTY_FEED_BUMP_CONFIRM_DATABASES=1 after confirming both databases are disposable"
database_table_count() {
  if command -v psql >/dev/null 2>&1; then
    psql "$1" --no-psqlrc --tuples-only --no-align --command "SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname NOT IN ('pg_catalog', 'information_schema')"
  else
    container_url=$(printf '%s\n' "$1" | sed -e 's/@127\.0\.0\.1:/@host.docker.internal:/' -e 's/@localhost:/@host.docker.internal:/')
    docker run --rm --add-host host.docker.internal:host-gateway pgvector/pgvector:0.8.6-pg17 psql "$container_url" --no-psqlrc --tuples-only --no-align --command "SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname NOT IN ('pg_catalog', 'information_schema')"
  fi
}
for database_url in "$TEST_DATABASE_URL" "$PLAYWRIGHT_DATABASE_URL"; do
  table_count=$(database_table_count "$database_url")
  [ "$table_count" = "0" ] || fail "each bump database must be empty; create new disposable databases before retrying"
done

current_version=$(awk '
  $0 == "[workspace.package]" { workspace = 1; next }
  workspace && /^\[/ { exit }
  workspace && $1 == "version" {
    gsub(/^[^\"]*\"|\"[^\"]*$/, "", $0)
    print
    exit
  }
' Cargo.toml)
printf '%s\n' "$current_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || fail "could not read a stable semantic version from [workspace.package]"

request=$1
case "$request" in
  major|minor|patch)
    target_version=$(printf '%s\n' "$current_version" | awk -F. -v bump="$request" '
      bump == "major" { print $1 + 1 ".0.0" }
      bump == "minor" { print $1 "." $2 + 1 ".0" }
      bump == "patch" { print $1 "." $2 "." $3 + 1 }
    ')
    ;;
  *)
    printf '%s\n' "$request" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || fail "version must be major, minor, patch, or X.Y.Z"
    target_version=$request
    ;;
esac

awk -v current="$current_version" -v target="$target_version" '
  BEGIN {
    split(current, a, ".")
    split(target, b, ".")
    for (i = 1; i <= 3; i++) {
      if (b[i] > a[i]) exit 0
      if (b[i] < a[i]) exit 1
    }
    exit 1
  }
' || fail "target version $target_version must be greater than current version $current_version"

printf 'Preparing Rusty Feed %s -> %s\n' "$current_version" "$target_version"
printf '\n==> Locked workspace metadata\n'
cargo metadata --locked --no-deps --format-version 1 >/dev/null
run_check "Patch whitespace" git diff --check
run_check "Rust formatting" cargo fmt --all --check
run_check "Clippy" cargo clippy --all-targets --all-features -- -D warnings
run_check "Workspace tests" cargo test --workspace --all-features
run_check "Node dependencies" npm ci
run_check "Playwright Chromium" npx playwright install chromium
run_check "Browser E2E" npm run test:e2e
run_check "TLS and HTTP/2" scripts/test-tls-proxy.sh
if command -v actionlint >/dev/null 2>&1; then
  run_check "GitHub Actions workflows" actionlint
else
  run_check "GitHub Actions workflows" docker run --rm -v "$repo_root:/repo:ro" --workdir /repo rhysd/actionlint:1.7.12 -color
fi
run_check "Release workspace build" cargo build --release --workspace

backup_dir=$(mktemp -d)
cp Cargo.toml "$backup_dir/Cargo.toml"
cp Cargo.lock "$backup_dir/Cargo.lock"
changed=0
cleanup() {
  status=$?
  trap - EXIT HUP INT TERM
  if [ "$status" -ne 0 ] && [ "$changed" -eq 1 ]; then
    cp "$backup_dir/Cargo.toml" Cargo.toml
    cp "$backup_dir/Cargo.lock" Cargo.lock
    printf '%s\n' "version bump failed; restored Cargo.toml and Cargo.lock" >&2
  fi
  rm -rf "$backup_dir"
  exit "$status"
}
trap 'exit 1' HUP INT TERM
trap cleanup EXIT
changed=1

CURRENT_VERSION=$current_version TARGET_VERSION=$target_version python3 <<'PY'
import os
from pathlib import Path

path = Path("Cargo.toml")
text = path.read_text()
start = text.index("[workspace.package]")
end = text.find("\n[", start + 1)
end = len(text) if end == -1 else end
section = text[start:end]
old = f'version = "{os.environ["CURRENT_VERSION"]}"'
new = f'version = "{os.environ["TARGET_VERSION"]}"'
if section.count(old) != 1:
    raise SystemExit("workspace version line was not unique")
path.write_text(text[:start] + section.replace(old, new, 1) + text[end:])
PY

cargo metadata --no-deps --format-version 1 >/dev/null
cargo metadata --locked --no-deps --format-version 1 | TARGET_VERSION=$target_version python3 -c '
import json, os, sys
target = os.environ["TARGET_VERSION"]
packages = json.load(sys.stdin)["packages"]
wrong = ["{} {}".format(package["name"], package["version"]) for package in packages if package["version"] != target]
if wrong:
    raise SystemExit("unexpected workspace versions: " + ", ".join(wrong))
'
cargo test -p rusty-feed-web --lib prefs::tests::exposes_application_version
changed=0
printf '\nRusty Feed version bumped to %s.\n' "$target_version"
