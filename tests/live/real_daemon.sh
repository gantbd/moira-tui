#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/moira-tui-live.XXXXXX")
fixture_pid=
cleanup() {
  if [ -n "$fixture_pid" ]; then
    kill "$fixture_pid" 2>/dev/null || true
    wait "$fixture_pid" 2>/dev/null || true
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

cargo run --quiet --manifest-path "$root/Cargo.toml" -p moira-tui-cli \
  --example real_daemon_fixture -- "$tmp/moira.db" "$tmp/moira.sock" \
  >"$tmp/daemon.out" 2>"$tmp/daemon.err" &
fixture_pid=$!

attempt=0
while [ ! -S "$tmp/moira.sock" ]; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 100 ]; then
    cat "$tmp/daemon.err" >&2
    exit 1
  fi
  sleep 0.05
done

output=$(cargo run --quiet --manifest-path "$root/Cargo.toml" -p moira-tui-cli \
  -- headless "$tmp/moira.sock")
wait "$fixture_pid"
fixture_pid=

printf '%s\n' "$output" | grep '"complete": true' >/dev/null
printf '%s\n' "$output" | grep '"capability": "sagas.inspect.v1"' >/dev/null
printf '%s\n' "$output" | grep '"phase": "ForwardPending"' >/dev/null
printf '%s\n' "$output" | grep '"total": 1' >/dev/null
printf '%s\n' "$output" | grep '"runnable_effects": 1' >/dev/null
