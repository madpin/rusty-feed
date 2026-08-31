#!/bin/sh
set -eu

test_root=$(mktemp -d)
container_name="rustss-caddy-tls-test-$$"
cleanup() {
  docker rm -f "$container_name" >/dev/null 2>&1 || true
  rm -rf "$test_root"
}
trap cleanup EXIT INT TERM

openssl req -x509 -newkey rsa:2048 -nodes -days 1 \
  -subj /CN=reader.example.test \
  -keyout "$test_root/key.pem" -out "$test_root/cert.pem" >/dev/null 2>&1

docker run --rm \
  -e RUSTSS_DOMAIN=reader.example.test \
  -e TLS_EMAIL=admin@example.test \
  -v "$PWD/docker/Caddyfile.auto:/etc/caddy/Caddyfile:ro" \
  caddy:2.11.4-alpine caddy validate --config /etc/caddy/Caddyfile

docker run -d --name "$container_name" -p 18443:443 \
  -e RUSTSS_DOMAIN=reader.example.test \
  -v "$PWD/docker/Caddyfile.custom:/etc/caddy/Caddyfile:ro" \
  -v "$test_root:/certs:ro" \
  caddy:2.11.4-alpine >/dev/null

attempt=0
while [ "$attempt" -lt 30 ]; do
  if printf '' | openssl s_client -connect 127.0.0.1:18443 \
      -servername reader.example.test -alpn h2 2>/dev/null \
      | grep -q 'ALPN protocol: h2'; then
    exit 0
  fi
  attempt=$((attempt + 1))
  sleep 1
done

docker logs "$container_name"
printf '%s\n' 'Caddy did not negotiate HTTP/2 through ALPN.' >&2
exit 1
