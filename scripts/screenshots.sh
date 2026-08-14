#!/usr/bin/env bash
# Visual baseline for the Tailwind port. See ui/scripts/screenshots.mjs for why
# it exists — short version: a utility-class migration fails silently, so a
# passing build proves nothing about how a screen looks.
#
#   scripts/screenshots.sh --snapshot                 (once) freeze the fixture
#   scripts/screenshots.sh baseline                   capture, labelled "baseline"
#   scripts/screenshots.sh tier1                      capture again after a tier
#   scripts/screenshots.sh --compare baseline tier1
#
# Determinism is the whole game: a baseline captured against live data diffs
# against itself the moment a run finishes or a chat moves. So `--snapshot`
# freezes a copy of the data dir's databases once, and every capture runs against
# a throwaway copy of that frozen fixture — your real data dir is only ever read,
# and only by --snapshot.
#
# Only the .db files are copied (~14M). `files/` and `local-runs/` are hundreds of
# megabytes of run artifacts that no screen here renders.
#
# The PNGs and the fixture contain your real project names and chat text. Both
# live under ui/.screenshots/, which is gitignored — keep it that way.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

fixture_pin="ui/.screenshots/fixture"
live_data="${ORX_LIVE_DATA_DIR:-$HOME/.local/share/openresearch}"

if [[ "${1:-}" == "--snapshot" ]]; then
  [[ -d "$live_data" ]] || { echo "no data dir at $live_data (set ORX_LIVE_DATA_DIR)" >&2; exit 1; }
  rm -rf "$fixture_pin"
  mkdir -p "$fixture_pin"
  cp "$live_data"/*.db* "$fixture_pin/" 2>/dev/null || true
  [[ -d "$live_data/idea-evaluator" ]] && cp -r "$live_data/idea-evaluator" "$fixture_pin/"
  echo "froze $(du -sh "$fixture_pin" | cut -f1) fixture from $live_data"
  echo "re-run --snapshot only when you WANT the baseline to move."
  exit 0
fi

if [[ "${1:-}" == "--compare" ]]; then
  exec node ui/scripts/screenshots.mjs --compare "${2:?labelA}" "${3:?labelB}"
fi

[[ -d "$fixture_pin" ]] || { echo "no fixture yet — run: scripts/screenshots.sh --snapshot" >&2; exit 1; }

label="${1:-baseline}"
port="${ORX_SCREENSHOT_PORT:-3399}"
# A FIXED path, not mktemp: Settings → Storage renders the data dir, so a random
# temp name would change that screenshot on every run all by itself.
workdir="$repo/ui/.screenshots/.run"

cleanup() {
  [[ -n "${server_pid:-}" ]] && kill "$server_pid" 2>/dev/null || true
}
trap cleanup EXIT
rm -rf "$workdir"
mkdir -p "$workdir"

# Copy, never use the pin directly: the server writes (WAL, logs) and a mutated
# pin would silently drift the baseline out from under later captures.
cp -r "$fixture_pin/." "$workdir/"

echo "==> building ui"
(cd ui && npm run build >/dev/null)

echo "==> building orx"
cargo build --quiet --bin orx

echo "==> starting orx up on :$port"
ORX_DATA_DIR="$workdir" ./target/debug/orx up \
  --port "$port" --no-browser --no-agent >"$workdir/orx-up.log" 2>&1 &
server_pid=$!

for _ in $(seq 1 60); do
  sleep 1
  curl -sf "http://127.0.0.1:$port/" >/dev/null 2>&1 && break
done
if ! curl -sf "http://127.0.0.1:$port/" >/dev/null 2>&1; then
  echo "orx up never came up; log:" >&2
  cat "$workdir/orx-up.log" >&2
  exit 1
fi

echo "==> capturing"
ORX_SCREENSHOT_URL="http://127.0.0.1:$port" node ui/scripts/screenshots.mjs "$label"
