#!/usr/bin/env bash
# End-to-end cloud tests against a local mock (needs mosquitto, mosquitto-clients, openssl).
set -euo pipefail
cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
chmod 755 "$tmp" # mosquitto drops privileges and must still read its certificate
cleanup() {
  pkill -P $$ 2>/dev/null || true
  pkill -f "$tmp/" 2>/dev/null || true
  pkill -f "mosquitto_sub -p 18883" 2>/dev/null || true
  sleep 0.3
  rm -rf "$tmp"
}
trap cleanup EXIT

openssl req -x509 -newkey rsa:2048 -nodes -keyout "$tmp/k.pem" -out "$tmp/c.pem" -days 2 -subj "/CN=127.0.0.1" >/dev/null 2>&1
chmod 644 "$tmp/k.pem" "$tmp/c.pem"
printf 'listener 18883 127.0.0.1\nallow_anonymous true\n' > "$tmp/plain.conf"
printf 'listener 18884 127.0.0.1\nallow_anonymous true\ncertfile %s\nkeyfile %s\n' "$tmp/c.pem" "$tmp/k.pem" > "$tmp/tls.conf"
mosquitto -c "$tmp/plain.conf" >"$tmp/plain.log" 2>&1 &
mosquitto -c "$tmp/tls.conf" >"$tmp/tls.log" 2>&1 &
sleep 1
python3 tests/mock_cloud.py >"$tmp/mock.log" 2>&1 &
sleep 1

NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 \
SB_MOCK_API=http://127.0.0.1:18080 SB_MOCK_TLS_PORT=18884 \
  cargo test --locked -- --ignored cloud::tests || { cat "$tmp"/*.log; exit 1; }
