#!/usr/bin/env bash
# Smoke test of a built FilmCraft on macOS: import → sequence → export → ffprobe, through the CLI
# (the same engine the app runs), plus the launch check of packaging/macos/check_symbols.py.
#
#   packaging/macos/smoke.sh [bin dir]      (default: target/release)
#
# Needs ffmpeg/ffprobe (test media and the output check) and jq. Exits non-zero on the first
# failed check. Nothing outside a temporary directory is written.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:-$ROOT/target/release}"
CLI="$BIN/filmcraft-cli"
OUT="$(mktemp -d "${TMPDIR:-/tmp}/filmcraft-smoke.XXXXXX")"
trap 'rm -rf "$OUT"' EXIT
pass() { printf '  ok    %s\n' "$*"; }
fail() { printf '  FAIL  %s\n' "$*" >&2; exit 1; }

echo "==> launch: framework symbols against the deployment target"
python3 -I "$ROOT/packaging/macos/check_symbols.py" "$BIN/filmcraft" || fail "a framework symbol is newer than the deployment target"
python3 -I "$ROOT/packaging/macos/check_symbols.py" "$CLI" || fail "a framework symbol of the CLI is newer than the deployment target"
pass "no symbol newer than the binaries' minos"

echo "==> MCP server"
# initialize + tools/list on stdio (headless): the server the app bundles for AI assistants
tools=$(printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | "$CLI" mcp --demo 2>/dev/null | jq -s '[.[] | select(.id == 2) | .result.tools[].name] | length')
[ "${tools:-0}" -ge 10 ] || fail "mcp tools/list returned ${tools:-nothing}"
pass "mcp: $tools tools"

echo "==> test media"
# 10 s of 1080p25 H.264 + stereo AAC with real detail (noise), a 4:2:2 10-bit ProRes clip and a
# 2-minute mono 24-bit WAV voice-over.
ffmpeg -v error -f lavfi -i "testsrc2=size=1920x1080:rate=25,noise=alls=30:allf=t" -f lavfi -i "sine=f=440:sample_rate=48000" \
  -t 10 -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -ac 2 "$OUT/camera.mp4"
ffmpeg -v error -f lavfi -i "testsrc2=size=1920x1080:rate=25" -t 3 -c:v prores_ks -profile:v 3 -pix_fmt yuv422p10le "$OUT/graded.mov"
ffmpeg -v error -f lavfi -i "sine=f=220:sample_rate=48000" -t 120 -ac 1 -c:a pcm_s24le "$OUT/vo.wav"
pass "camera.mp4, graded.mov, vo.wav"

echo "==> probe"
for f in camera.mp4 graded.mov vo.wav; do
  "$CLI" probe "$OUT/$f" --compact >/dev/null || fail "probe $f"
done
pass "all three probe"

echo "==> import and build a sequence"
P="$OUT/smoke.fcproj"
"$CLI" --compact import "$OUT/camera.mp4" "$OUT/graded.mov" "$OUT/vo.wav" --save-as "$P" >"$OUT/import.json" || fail "import"
items="$(jq -c '[.. | objects | select(has("items")) | .items[]] | unique' "$OUT/import.json")"
[ "$(jq length <<<"$items")" = 3 ] || fail "import added $(jq length <<<"$items") items, expected 3 ($(cat "$OUT/import.json"))"
camera="$(jq '.[0]' <<<"$items")"
"$CLI" --project "$P" --save --compact exec file.newSequenceFromClip "{\"items\":[$camera]}" >/dev/null || fail "sequence from clip"
pass "3 items imported, sequence from camera.mp4"
# importing the same files again adds nothing (#356)
"$CLI" --project "$P" --compact import "$OUT/camera.mp4" "$OUT/vo.wav" >"$OUT/again.json" || fail "re-import"
[ "$(jq '[.. | objects | select(has("items")) | .items[]] | length' "$OUT/again.json")" = 0 ] || fail "re-import added items: $(cat "$OUT/again.json")"
pass "re-import of the same files adds nothing"

echo "==> render a frame"
"$CLI" --project "$P" render --seconds 5 --out "$OUT/frame.png" 2>/dev/null || fail "render"
[ -s "$OUT/frame.png" ] || fail "empty frame"
pass "frame at 5 s"

echo "==> export H.264, 20 Mbps VBR"
"$CLI" --project "$P" export "$OUT/out.mp4" --format h264 --settings '{"bitrateKbps":20000,"bitrateMode":"vbr1Pass"}' >/dev/null 2>&1 || fail "export"
dur="$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$OUT/out.mp4")"
vcodec="$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name -of csv=p=0 "$OUT/out.mp4")"
acodec="$(ffprobe -v error -select_streams a:0 -show_entries stream=codec_name -of csv=p=0 "$OUT/out.mp4")"
vbits="$(ffprobe -v error -select_streams v:0 -show_entries stream=bit_rate -of csv=p=0 "$OUT/out.mp4")"
[ "$vcodec" = h264 ] || fail "video codec $vcodec"
[ "$acodec" = aac ] || fail "audio codec $acodec"
python3 -c "import sys; d=float(sys.argv[1]); sys.exit(0 if abs(d-10)<0.2 else 1)" "$dur" || fail "duration $dur s, expected 10"
ffmpeg -v error -xerror -i "$OUT/out.mp4" -f null - || fail "ffmpeg reports decode errors"
pass "h264 + aac, ${dur} s, decodes cleanly, video $((vbits / 1000)) kbps for a 20000 kbps target"

echo "==> export ProRes"
"$CLI" --project "$P" export "$OUT/out.mov" --format prores >/dev/null 2>&1 || fail "prores export"
[ "$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name -of csv=p=0 "$OUT/out.mov")" = prores ] || fail "not prores"
pass "ProRes 422 HQ"

echo "smoke: all checks passed"
