#!/usr/bin/env bash
# Tests the scripts that the built-in indico package ships in its skills
# (plugins/builtin/indico/skills/indico/*/scripts), with no network: a stub `aurora-agent` first on
# PATH answers from the recorded-shape fixtures in scripts/plugins/fixtures/indico/ (every value is
# made up) and logs its arguments. Needs bash, jq and python3.
#
#   bash scripts/plugins/indico_skills_test.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SK="$ROOT/plugins/builtin/indico/skills/indico"
FIX="$ROOT/scripts/plugins/fixtures/indico"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export TMPDIR="$WORK/tmp"
mkdir -p "$TMPDIR" "$WORK/bin"
export STUB_FIX="$FIX" STUB_LOG="$WORK/calls.log"
unset INDICO_ACCOUNT SKILLS_DIR

# The stub: `aurora-agent call indico <operation> [name=value ...] [--account A] [--raw]`.
# It answers fixtures/<operation>[-<date of start_dt>].json and records every argument on one line.
cat > "$WORK/bin/aurora-agent" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$STUB_LOG"
[ "${1:-}" = call ] && [ "${2:-}" = indico ] || { echo "stub: not an indico call: $*" >&2; exit 2; }
op=$3; shift 3
day=""
for a in "$@"; do
  case "$a" in start_dt=*) day=${a#start_dt=}; day=${day%%T*} ;; esac
done
if [ -n "${STUB_FAIL:-}" ]; then echo "stub: forced failure" >&2; exit 1; fi
if [ -n "$day" ] && [ -f "$STUB_FIX/$op-$day.json" ]; then cat "$STUB_FIX/$op-$day.json"
elif [ -f "$STUB_FIX/$op.json" ]; then cat "$STUB_FIX/$op.json"
else echo "stub: no fixture for $op" >&2; exit 1; fi
STUB
chmod +x "$WORK/bin/aurora-agent"
export PATH="$WORK/bin:$PATH"

fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1" >&2; fails=$((fails + 1)); }
expect_eq() { # name, want, got
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; echo "--- want" >&2; echo "$2" >&2; echo "--- got" >&2; echo "$3" >&2; fi
}
expect_has() { # name, needle, text
  if grep -qF -- "$2" <<<"$3"; then ok "$1"; else bad "$1: missing '$2' in:"; echo "$3" >&2; fi
}

# --- the scripts are executable and clean ---------------------------------------------------------
for f in "$SK"/*/scripts/*; do
  [ -x "$f" ] || bad "not executable: $f"
done
if command -v shellcheck >/dev/null 2>&1; then
  shellcheck "$SK"/indico-find-and-book-room/scripts/free-rooms "$SK"/indico-book-recurring-room/scripts/series-free \
    && ok "shellcheck" || bad "shellcheck"
else
  echo "skip shellcheck (not installed)"
fi
for f in "$SK"/indico-meeting-prep/scripts/event-brief "$SK"/indico-meeting-prep/scripts/doc2text; do
  python3 -I -c 'import ast,sys; ast.parse(open(sys.argv[1]).read())' "$f" && ok "python syntax $(basename "$f")" || bad "python syntax $f"
done

# --- free-rooms: joined with the room list, sorted, filtered by building -----------------------
FREE="$SK/indico-find-and-book-room/scripts/free-rooms"
out=$("$FREE" 2026-12-01 10:00 11:00 10)
want=$'11\t10/R-001\t12\tdirect\tProjector and video conferencing\n13\t20/S-010\t10\tdirect\t\n14\t20/S-011\t20\tdirect\tNOT A MEETING ROOM: coffee area\n12\t10/R-002\t30\tNEEDS-CONFIRMATION\tPre-booking only'
expect_eq "free-rooms: joined, direct first, by building and capacity" "$want" "$out"
out=$("$FREE" 2026-12-01 10:00 11:00 10 20)
want=$'13\t20/S-010\t10\tdirect\t\n14\t20/S-011\t20\tdirect\tNOT A MEETING ROOM: coffee area'
expect_eq "free-rooms: building filter" "$want" "$out"
calls=$(cat "$STUB_LOG")
expect_has "free-rooms: one search without building" "call indico search_rooms capacity=10 start_dt=2026-12-01T10:00:00 end_dt=2026-12-01T11:00:00 repeat_frequency=NEVER --raw" "$calls"
if grep -q "building=" <<<"$calls"; then bad "free-rooms: sent a building filter"; else ok "free-rooms: no building parameter sent"; fi
n=$(grep -c "list_all_rooms" "$STUB_LOG")
expect_eq "free-rooms: room list read once, then cached" 1 "$n"
if "$FREE" 2026-12-01 10:00 11:00 ten 2>/dev/null; then bad "free-rooms: accepted a bad capacity"; else ok "free-rooms: bad capacity refused"; fi
if "$FREE" "2026-12-01; rm -rf x" 10:00 11:00 10 2>/dev/null; then bad "free-rooms: accepted a bad date"; else ok "free-rooms: bad date refused"; fi
: > "$STUB_LOG"
INDICO_ACCOUNT=B "$FREE" 2026-12-01 10:00 11:00 10 >/dev/null
expect_has "free-rooms: INDICO_ACCOUNT becomes --account" "--account B" "$(cat "$STUB_LOG")"
if STUB_FAIL=1 "$FREE" 2026-12-02 10:00 11:00 10 2>/dev/null; then bad "free-rooms: ignored a failing search"; else ok "free-rooms: a failing call exits non-zero"; fi

# --- series-free: the per-date matrix -------------------------------------------------------------
SERIES="$SK/indico-book-recurring-room/scripts/series-free"
out=$("$SERIES" 2026-12-01 2026-12-15 14:30 15:30 10 1)
want=$'dates checked: 3 (2026-12-01 .. 2026-12-15)\n13\t20/S-010\tcap 10\tdirect\tfree 3/3\tbusy: -\n12\t10/R-002\tcap 30\tNEEDS-CONFIRMATION\tfree 3/3\tbusy: -\n11\t10/R-001\tcap 12\tdirect\tfree 2/3\tbusy: 2026-12-08\n14\t20/S-011\tcap 20\tdirect\tfree 2/3\tbusy: 2026-12-08'
expect_eq "series-free: matrix over three Tuesdays" "$want" "$out"
out=$("$SERIES" 2026-12-01 2026-12-15 14:30 15:30 10 1 10)
expect_has "series-free: building filter keeps building 10 only" "10/R-001" "$out"
if grep -q "20/S" <<<"$out"; then bad "series-free: building filter leaked"; else ok "series-free: building filter excludes the others"; fi
out=$("$SERIES" 2026-12-01 2026-12-29 14:30 15:30 10 2)
expect_has "series-free: every other week" "dates checked: 3 (2026-12-01 .. 2026-12-29)" "$out"
if "$SERIES" 2026-12-15 2026-12-01 14:30 15:30 10 1 2>/dev/null; then bad "series-free: accepted last before first"; else ok "series-free: last before first refused"; fi
if "$SERIES" 2026-01-01 2030-01-01 14:30 15:30 10 1 2>/dev/null; then bad "series-free: accepted over 60 dates"; else ok "series-free: more than 60 dates refused"; fi
if [ -n "$(find "$TMPDIR/aurora-agent" -maxdepth 1 -name 'series.*' 2>/dev/null)" ]; then bad "series-free: left its temp directory"; else ok "series-free: temp directory removed"; fi

# --- event-brief: sections and the download arguments ---------------------------------------------
BRIEF="$SK/indico-meeting-prep/scripts/event-brief"
: > "$STUB_LOG"
out=$("$BRIEF" 4242)
expect_has "event-brief: title" "# Weekly team meeting" "$out"
expect_has "event-brief: when and where" "When: 2026-11-24 14:30 - 2026-11-24 16:00 Europe/Zurich" "$out"
expect_has "event-brief: chairs" "Chairs: Pat Chair" "$out"
expect_has "event-brief: URL" "URL: https://indico.example.org/event/4242/" "$out"
expect_has "event-brief: description text" "- Welcome" "$out"
expect_has "event-brief: links section" "- https://notes.example.org/abc" "$out"
expect_has "event-brief: agenda count and material" "## Agenda (2 items, material on 1)" "$out"
expect_has "event-brief: speaker" "- 14:35 Status of the upgrade (20 min) - Sam Speaker" "$out"
expect_has "event-brief: unscheduled item last" "- unscheduled Late item (10 min) - no speaker" "$out"
expect_has "event-brief: talk minutes" "[minutes] Action: Sam to update the plan." "$out"
expect_has "event-brief: material download arguments" "-> get_event_file event=4242 folder=910 attachment=911.bin" "$out"
expect_has "event-brief: link attachment" "- [link] Recording: https://video.example.org/r/1" "$out"
expect_has "event-brief: event minutes" "Decision: ship on Friday." "$out"
expect_has "event-brief: event attachment" "-> get_event_file event=4242 folder=900 attachment=901.bin" "$out"
expect_has "event-brief: one get_event call" "call indico get_event event=4242.json detail=contributions --raw" "$(cat "$STUB_LOG")"
if "$BRIEF" "4242; id" 2>/dev/null; then bad "event-brief: accepted a non-numeric id"; else ok "event-brief: non-numeric id refused"; fi
if STUB_FAIL=1 "$BRIEF" 4242 2>/dev/null; then bad "event-brief: ignored a failing call"; else ok "event-brief: a failing call exits non-zero"; fi

# --- doc2text on a PPTX and a DOCX made here -------------------------------------------------------
D2T="$SK/indico-meeting-prep/scripts/doc2text"
python3 -I - "$WORK" <<'PY'
import sys, zipfile
w = sys.argv[1]
A = 'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
def slide(*lines):
    ps = "".join(f"<a:p><a:r><a:t>{t}</a:t></a:r></a:p>" for t in lines)
    return f'<?xml version="1.0"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" {A}>{ps}</p:sld>'
with zipfile.ZipFile(f"{w}/deck.pptx", "w") as z:
    z.writestr("ppt/slides/slide1.xml", slide("Upgrade status", "Phase one done"))
    z.writestr("ppt/slides/slide2.xml", slide("Next steps"))
paras = "".join(f"<w:p><w:r><w:t>{t}</w:t></w:r></w:p>" for t in ("Minutes", "Ship on Friday"))
with zipfile.ZipFile(f"{w}/minutes.docx", "w") as z:
    z.writestr("word/document.xml", '<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>' + paras + "</w:body></w:document>")
open(f"{w}/note.txt", "w").write("plain text file\n")
open(f"{w}/old.doc", "wb").write(b"\xd0\xcf\x11\xe0" + b"\0" * 32)
PY
out=$("$D2T" "$WORK/deck.pptx")
want=$'--- slide 1\nUpgrade status\nPhase one done\n--- slide 2\nNext steps'
expect_eq "doc2text: PPTX slides in order" "$want" "$out"
out=$("$D2T" "$WORK/minutes.docx")
expect_eq "doc2text: DOCX paragraphs" $'Minutes\nShip on Friday' "$out"
out=$("$D2T" "$WORK/deck.pptx" 20)
expect_has "doc2text: says when it cuts" "[doc2text: cut at 20 of" "$out"
expect_eq "doc2text: plain text passes through" "plain text file" "$("$D2T" "$WORK/note.txt")"
if "$D2T" "$WORK/old.doc" 2>/dev/null; then bad "doc2text: accepted an old binary format"; else ok "doc2text: old .doc refused"; fi
printf '%%PDF-1.4\n' > "$WORK/a.pdf"
if python3 -I -c 'import pypdf' 2>/dev/null; then
  echo "skip doc2text PDF without a reader (pypdf is installed)"
elif PATH="$WORK/bin:/nonexistent" "$(command -v python3)" -I "$D2T" "$WORK/a.pdf" 2>"$WORK/err" >/dev/null; then
  bad "doc2text: read a PDF with no reader"
else
  expect_has "doc2text: no PDF reader says so" "cannot read PDF" "$(cat "$WORK/err")"
fi

# --- no script reaches the network other than through aurora-agent --------------------------------
if grep -nE '\b(curl|wget)\b|requests\.|urllib|http\.client|import socket' "$SK"/*/scripts/*; then
  bad "a script uses the network directly"
else
  ok "scripts use aurora-agent only"
fi

if [ "$fails" -ne 0 ]; then echo "$fails check(s) failed" >&2; exit 1; fi
echo "indico skill scripts: all checks passed"
