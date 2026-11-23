#!/usr/bin/env bash
# Run scopinator's Alpaca backend against the ASCOM Alpaca Omni Simulators
# (OmniSim), the reference simulator most Alpaca clients are tested against.
#
#   tools/alpaca-sim.sh            download if needed, start, run the tests, stop
#   tools/alpaca-sim.sh fetch      download and unpack the pinned release only
#   tools/alpaca-sim.sh start      start the simulator and leave it running
#   tools/alpaca-sim.sh stop       stop a simulator left running by `start`
#   tools/alpaca-sim.sh test       run the tests against a running simulator
#
# The release is pinned and its checksum verified, the simulator is bound to
# 127.0.0.1, and it gets an empty home directory each time it starts, so every
# run begins from the simulator's default state and nothing is written to
# your own ~/.ASCOM.
#
# Environment:
#   ALPACA_SIM_PORT   port to bind (default 32323)
#   ALPACA_SIM_CACHE  where the release is kept (default ~/.cache/scopinator/omnisim)
set -euo pipefail

VERSION="v0.4.0"
REPO="ASCOMInitiative/ASCOM.Alpaca.Simulators"
PORT="${ALPACA_SIM_PORT:-32323}"
CACHE="${ALPACA_SIM_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/scopinator/omnisim}"
URL="http://127.0.0.1:${PORT}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
    ASSET="ascom.alpaca.simulators.linux-x64"
    SHA256="c004a13c869473f4e2f110a231c2bcadbaa43ccabc324deb47bb7f2e79a5d6a6"
    ;;
  Linux-aarch64)
    ASSET="ascom.alpaca.simulators.linux-aarch64"
    SHA256="" # not pinned: this platform has not been run by the maintainers
    ;;
  *)
    echo "alpaca-sim: no pinned OmniSim build for $(uname -s) $(uname -m)" >&2
    exit 2
    ;;
esac

DIR="$CACHE/$VERSION"
BIN="$DIR/$ASSET/ascom.alpaca.simulators"
RUN="$CACHE/run"
PIDFILE="$RUN/sim.pid"
LOG="$RUN/sim.log"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

fetch() {
  if [[ -x "$BIN" ]]; then
    return
  fi
  mkdir -p "$DIR"
  local archive="$DIR/$ASSET.tar.xz"
  echo "alpaca-sim: downloading OmniSim $VERSION ($ASSET)"
  curl -fL --retry 3 -o "$archive" \
    "https://github.com/$REPO/releases/download/$VERSION/$ASSET.tar.xz"
  if [[ -n "$SHA256" ]]; then
    echo "$SHA256  $archive" | sha256sum --check --quiet
  else
    echo "alpaca-sim: warning: no pinned checksum for $ASSET; not verified" >&2
  fi
  tar -xJf "$archive" -C "$DIR"
  rm -f "$archive"
  [[ -x "$BIN" ]] || { echo "alpaca-sim: $BIN is missing after unpacking" >&2; exit 1; }
}

running() {
  [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null
}

start() {
  fetch
  if running; then
    echo "alpaca-sim: already running (pid $(cat "$PIDFILE")) at $URL"
    return
  fi
  if curl -sf -o /dev/null "$URL/management/v1/configureddevices"; then
    echo "alpaca-sim: something else is already answering at $URL" >&2
    exit 1
  fi
  # A fresh home each start: the simulator keeps its settings there, and the
  # tests assume its defaults.
  rm -rf "$RUN"
  mkdir -p "$RUN/home"
  (
    cd "$(dirname "$BIN")"
    HOME="$RUN/home" nohup "$BIN" "--urls=$URL" >"$LOG" 2>&1 &
    echo $! >"$PIDFILE"
  )
  for _ in $(seq 1 120); do
    if curl -sf -o /dev/null "$URL/management/v1/configureddevices"; then
      echo "alpaca-sim: OmniSim $VERSION is up at $URL (pid $(cat "$PIDFILE"), log $LOG)"
      return
    fi
    if ! running; then
      echo "alpaca-sim: the simulator exited while starting; see $LOG" >&2
      tail -20 "$LOG" >&2 || true
      exit 1
    fi
    sleep 0.5
  done
  echo "alpaca-sim: the simulator did not answer at $URL within 60 s; see $LOG" >&2
  stop
  exit 1
}

stop() {
  if running; then
    local pid
    pid="$(cat "$PIDFILE")"
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 20); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.25
    done
    kill -9 "$pid" 2>/dev/null || true
    echo "alpaca-sim: stopped (pid $pid)"
  fi
  rm -f "$PIDFILE"
}

run_tests() {
  cd "$ROOT"
  # One test at a time: they share one simulated telescope.
  ALPACA_SIM_URL="$URL" cargo test -p scopinator --features alpaca \
    --test alpaca_omnisim -- --ignored --test-threads=1 "$@"
}

case "${1:-all}" in
  fetch) fetch ;;
  start) start ;;
  stop) stop ;;
  test) shift; run_tests "$@" ;;
  all)
    start
    trap stop EXIT
    run_tests
    ;;
  *)
    echo "usage: $0 [fetch|start|stop|test|all]" >&2
    exit 2
    ;;
esac
