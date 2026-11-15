#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd -P)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/moira-tui-live.XXXXXX")
fixture="$root/target/debug/examples/real_daemon_fixture"
client="$root/target/debug/moira-tui"
fixture_pid=

cleanup() {
  if [ -n "$fixture_pid" ] && kill -0 "$fixture_pid" 2>/dev/null; then
    kill "$fixture_pid" 2>/dev/null || true
    wait "$fixture_pid" 2>/dev/null || true
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT HUP INT TERM

cargo build --manifest-path "$root/Cargo.toml" -p moira-tui-cli \
  --bin moira-tui --example real_daemon_fixture --locked --offline

start_fixture() {
  name=$1
  socket="$tmp/$name.sock"
  "$fixture" "$tmp/$name.db" "$socket" >"$tmp/$name.out" 2>"$tmp/$name.err" &
  fixture_pid=$!
  attempt=0
  while [ ! -S "$socket" ]; do
    attempt=$((attempt + 1))
    if ! kill -0 "$fixture_pid" 2>/dev/null || [ "$attempt" -ge 100 ]; then
      sed -n '1,160p' "$tmp/$name.err" >&2
      exit 1
    fi
    sleep 0.05
  done
}

finish_fixture() {
  wait "$fixture_pid"
  fixture_pid=
}

start_fixture headless
output=$("$client" headless "$tmp/headless.sock")
finish_fixture
printf '%s\n' "$output" | grep '"complete": true' >/dev/null
printf '%s\n' "$output" | grep '"capability": "sagas.inspect.v1"' >/dev/null
printf '%s\n' "$output" | grep '"phase": "FencedUncertain"' >/dev/null
printf '%s\n' "$output" | grep '"total": 1' >/dev/null
printf '%s\n' "$output" | grep '"fenced": 1' >/dev/null

start_fixture interactive
python3 - "$tmp" "$client" <<'PYTHON'
from pathlib import Path
import shlex
import subprocess
import sys
import time

scratch, binary = sys.argv[1:]
tmux_socket = str(Path(scratch) / "tmux.sock")
command = shlex.join([binary, "interactive", str(Path(scratch) / "interactive.sock")])

def tmux(*args, check=True):
    return subprocess.run(
        ["tmux", "-S", tmux_socket, "-f", "/dev/null", *args],
        capture_output=True, text=True, check=check, timeout=5,
    )

def expect(*markers):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        screen = tmux("capture-pane", "-p", "-t", "moira").stdout
        if all(marker in screen for marker in markers):
            return
        time.sleep(0.025)
    raise AssertionError(f"Missing {markers!r} in:\n{screen}")

try:
    tmux("new-session", "-d", "-x", "120", "-y", "24", "-s", "moira", command)
    expect("census total 1", "FencedUncertain")
    tmux("send-keys", "-t", "moira", "Enter")
    expect("evidence page: 1", "operator receipts: 1", "authority 9/10")
    tmux("send-keys", "-t", "moira", "q")
    deadline = time.monotonic() + 5
    while tmux("has-session", "-t", "moira", check=False).returncode == 0:
        if time.monotonic() >= deadline:
            raise AssertionError("Moira TUI did not exit")
        time.sleep(0.025)
finally:
    tmux("kill-server", check=False)
PYTHON
finish_fixture

echo 'PASS: real Moira daemon -> census plus selected-saga evidence and operator receipts'
