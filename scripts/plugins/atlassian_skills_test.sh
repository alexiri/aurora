#!/usr/bin/env bash
# Tests the two scripts that the built-in atlassian package ships in its jira-dc skills
# (plugins/builtin/atlassian/skills/jira-dc/*/scripts: jira-find-similar and md2jira), with no network:
# a stub `aurora-agent` first on PATH records its arguments (one file per call, NUL-separated) and
# answers canned files from scripts/plugins/fixtures/atlassian/ (made-up issues in the shape of Jira
# Data Center's GET /rest/api/2/search). Needs bash, jq and python3.
#
#   bash scripts/plugins/atlassian_skills_test.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SK="$ROOT/plugins/builtin/atlassian/skills/jira-dc"
FIND="$SK/jira-dc-search/scripts/jira-find-similar"
MD="$SK/jira-dc-create-issue/scripts/md2jira"
FIX="$ROOT/scripts/plugins/fixtures/atlassian"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export TMPDIR="$WORK/tmp"
mkdir -p "$TMPDIR" "$WORK/bin" "$WORK/calls"
export STUB_FIX="$FIX" STUB_CALLS="$WORK/calls"
unset STUB_FAIL

# The stub: `aurora-agent call jira-dc search_issues jql=... fields=... maxResults=N [--account A] --out FILE`.
cat > "$WORK/bin/aurora-agent" <<'STUB'
#!/usr/bin/env bash
n=$(find "$STUB_CALLS" -type f | wc -l)
printf '%s\0' "$@" > "$STUB_CALLS/call-$((n + 1))"
[ "${1:-}" = call ] && [ "${2:-}" = jira-dc ] && [ "${3:-}" = search_issues ] || { echo "stub: not a search_issues call: $*" >&2; exit 2; }
shift 3
out=""; jql=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out=$2; shift ;;
    --account) shift ;;
    jql=*) jql=${1#jql=} ;;
  esac
  shift
done
[ -n "$out" ] || { echo "stub: --out missing" >&2; exit 2; }
case "$out" in /*) ;; *) echo "stub: --out must be absolute: $out" >&2; exit 2 ;; esac
[ "$(stat -c %a "$(dirname "$(dirname "$out")")")" = 700 ] || { echo "stub: temp dir is not mode 0700" >&2; exit 2; }
if [ -n "${STUB_FAIL:-}" ]; then cat "$STUB_FIX/search_error.txt" >&2; exit 1; fi
case "$jql" in
  *"labels ="*) cp "$STUB_FIX/search_label.json" "$out" ;;
  *"text ~"*) cp "$STUB_FIX/search_words.json" "$out" ;;
  *) echo "stub: no fixture for $jql" >&2; exit 1 ;;
esac
STUB
chmod +x "$WORK/bin/aurora-agent"
export PATH="$WORK/bin:$PATH"

fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1" >&2; fails=$((fails + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; echo "--- want" >&2; echo "$2" >&2; echo "--- got" >&2; echo "$3" >&2; fi; }
expect_has() { if grep -qF -- "$2" <<<"$3"; then ok "$1"; else bad "$1: missing '$2' in:"; echo "$3" >&2; fi; }
expect_not() { if grep -qF -- "$2" <<<"$3"; then bad "$1: found '$2' in:"; echo "$3" >&2; else ok "$1"; fi; }
reset() { rm -f "$WORK"/calls/*; }
calls() { find "$WORK/calls" -type f | wc -l | tr -d ' '; }
# The argument "$2" (a prefix such as jql=) of call number "$1", exactly as aurora-agent received it.
arg() { python3 -I -c 'import sys
a = open(sys.argv[1], "rb").read().split(b"\0")[:-1]
m = [x for x in a if x.startswith(sys.argv[2].encode())]
sys.stdout.write(m[0].decode() if m else "")' "$WORK/calls/call-$1" "$2"; }
F() { "$FIND" "$@"; }
rc_of() { local rc=0; "$@" >/dev/null 2>&1 || rc=$?; echo "$rc"; }

for s in "$FIND" "$MD"; do
  if [ -x "$s" ]; then ok "$(basename "$s") is executable"; else bad "$(basename "$s") is not executable"; fi
  if python3 -I -m py_compile "$s"; then ok "py_compile $(basename "$s")"; else bad "py_compile $(basename "$s")"; fi
done
find "$SK" -name __pycache__ -prune -exec rm -rf {} + 2>/dev/null || true

# === jira-find-similar ===================================================================================
OPEN=' AND statusCategory != Done'
TAIL=' ORDER BY updated DESC'

reset
out=$(F --project ABC --label ops-agent)
expect_eq "label search: one call" 1 "$(calls)"
expect_eq "label search: the exact JQL" "jql=project = ABC AND labels = \"ops-agent\"$OPEN$TAIL" "$(arg 1 jql=)"
expect_eq "label search: fields" "fields=summary,status,updated,labels" "$(arg 1 fields=)"
expect_eq "label search: default maxResults" "maxResults=50" "$(arg 1 maxResults=)"
want=$'## issues labelled ops-agent\n# jql: project = ABC AND labels = "ops-agent" AND statusCategory != Done ORDER BY updated DESC\n# 2 found (showing 2)\nABC-12\tIn Progress\t2026-11-20\tops-agent,certs\tRotate the demo certificates\nABC-7\tOpen\t2026-11-18\tops-agent\tClean up the old demo share'
expect_eq "label search: the printed report" "$want" "$out"
out_path=$(python3 -I -c 'import sys;a=open(sys.argv[1],"rb").read().split(b"\0");print(a[a.index(b"--out")+1].decode())' "$WORK/calls/call-1")
case "$out_path" in "$TMPDIR"/aurora-agent/*) ok "--out is absolute, under TMPDIR/aurora-agent" ;; *) bad "--out is not under TMPDIR/aurora-agent: $out_path" ;; esac
if [ -e "$out_path" ]; then bad "temp result file left behind"; else ok "temp result file removed"; fi

reset
out=$(F --project ABC 'two words' 'a"b\c' 'disk/full*')
expect_eq "three words: one call" 1 "$(calls)"
expect_eq "three words: the exact JQL (phrase, quote and backslash dropped, reserved characters dropped)" \
  "jql=project = ABC$OPEN AND (text ~ \"\\\"two words\\\"\" OR text ~ \"\\\"a b c\\\"\" OR text ~ \"\\\"disk full\\\"\")$TAIL" "$(arg 1 jql=)"
expect_has "words: total and shown" "# 120 found (showing 3)" "$out"
expect_has "words: a line" $'ABC-29\tWaiting\t2026-11-17\tstorage\tDemo host out of disk space' "$out"
expect_has "words: an issue without labels" $'ABC-31\tOpen\t2026-11-21\t\t' "$out"
expect_has "words: control characters in a summary are replaced" $'Widget sync fails?[31m red? on host demo-01' "$out"
if grep -q $'\x1b\|\x07' <<<"$out"; then bad "control characters reached the output"; else ok "no control characters in the output"; fi
expect_has "words: the JQL is printed for the report" "# jql: project = ABC AND statusCategory != Done AND (text ~" "$out"

reset
F --project ABC --include-done old >/dev/null
expect_eq "--include-done drops the statusCategory clause" "jql=project = ABC AND (text ~ \"old\")$TAIL" "$(arg 1 jql=)"
reset
F --project ABC --include-done --label x >/dev/null
expect_eq "--include-done with a label" "jql=project = ABC AND labels = \"x\"$TAIL" "$(arg 1 jql=)"
reset
F --project ABC --label x demo --account B --max 7 >/dev/null
expect_eq "label and words: two calls" 2 "$(calls)"
expect_eq "--account is passed through (call 1)" "B" "$(python3 -I -c 'import sys;a=open(sys.argv[1],"rb").read().split(b"\0");print(a[a.index(b"--account")+1].decode())' "$WORK/calls/call-1")"
expect_eq "--account is passed through (call 2)" "B" "$(python3 -I -c 'import sys;a=open(sys.argv[1],"rb").read().split(b"\0");print(a[a.index(b"--account")+1].decode())' "$WORK/calls/call-2")"
expect_eq "--max 7 reaches maxResults" "maxResults=7" "$(arg 2 maxResults=)"
expect_eq "no --account: none sent" "" "$(reset; F --project ABC demo >/dev/null; tr '\0' ' ' < "$WORK/calls/call-1" | grep -o -- '--account' || true)"

# Hostile words: each stays one text clause inside one quoted string and adds no condition.
hostile() { # name word expected-clause
  reset
  F --project ABC "$2" >/dev/null 2>&1 || true
  local got; got=$(arg 1 jql=)
  expect_eq "hostile word ($1): the exact JQL" "jql=project = ABC$OPEN AND ($3)$TAIL" "$got"
  if python3 -I -c 'import sys;sys.exit(0 if "\n" in sys.argv[1] else 1)' "$got"; then bad "hostile word ($1): a newline reached the JQL"; else ok "hostile word ($1): no newline in the JQL"; fi
}
hostile 'quote'        'a" OR project = SECRET OR text ~ "b' 'text ~ "\"a OR project = SECRET OR text b\""'
hostile 'backslash'    'a\"' 'text ~ "a"'
hostile 'OR'           'OR' 'text ~ "\"OR\""'
hostile 'lowercase and' 'and' 'text ~ "\"and\""'
hostile 'paren'        'x) OR (project = Z' 'text ~ "\"x OR project = Z\""'
hostile 'star'         'foo*' 'text ~ "foo"'
hostile 'newline'      $'line one\nproject = Z' 'text ~ "\"line one project = Z\""'
hostile 'colon'        'summary:secret' 'text ~ "\"summary secret\""'
hostile 'slash path'   '/mnt/data1' 'text ~ "\"mnt data1\""'
hostile 'brackets'     '{a}[b]^~' 'text ~ "\"a b\""'
for w in ')' '*' '"' $'\\' $'\n' '---'; do
  reset
  rc=0; F --project ABC "$w" >/dev/null 2>&1 || rc=$?
  expect_eq "a word with nothing searchable ($(printf %q "$w")) is refused or skipped: exit 2, no call" "2 0" "$rc $(calls)"
done
reset
err=$(F --project ABC ')' demo 2>&1 >/dev/null)
expect_has "an empty word is skipped with a stderr note" "skipped a word" "$err"
expect_eq "the other word is still searched" "jql=project = ABC$OPEN AND (text ~ \"demo\")$TAIL" "$(arg 1 jql=)"
reset
F --project ABC --label 'a\b' >/dev/null
expect_eq "label with a backslash is escaped for JQL" "jql=project = ABC AND labels = \"a\\\\b\"$OPEN$TAIL" "$(arg 1 jql=)"

# Usage errors exit 2 and make no call.
reset
expect_eq "bad project key (lower case)" 2 "$(rc_of "$FIND" --project abc demo)"
expect_eq "bad project key (JQL in it)" 2 "$(rc_of "$FIND" --project 'ABC OR x' demo)"
expect_eq "no project" 2 "$(rc_of "$FIND" demo)"
expect_eq "no label and no words" 2 "$(rc_of "$FIND" --project ABC)"
expect_eq "--max 0" 2 "$(rc_of "$FIND" --project ABC --max 0 demo)"
expect_eq "--max 201" 2 "$(rc_of "$FIND" --project ABC --max 201 demo)"
expect_eq "--max not a number" 2 "$(rc_of "$FIND" --project ABC --max x demo)"
expect_eq "label with a space" 2 "$(rc_of "$FIND" --project ABC --label 'a b')"
expect_eq "label with a quote" 2 "$(rc_of "$FIND" --project ABC --label 'a"b')"
expect_eq "label with a newline" 2 "$(rc_of "$FIND" --project ABC --label $'a\nb')"
expect_eq "empty label" 2 "$(rc_of "$FIND" --project ABC --label '')"
expect_eq "label of 256 characters" 2 "$(rc_of "$FIND" --project ABC --label "$(printf 'a%.0s' $(seq 1 256))")"
expect_eq "empty word" 2 "$(rc_of "$FIND" --project ABC '')"
expect_eq "word of 101 characters" 2 "$(rc_of "$FIND" --project ABC "$(printf 'a%.0s' $(seq 1 101))")"
words21=$(seq -f 'w%g' 1 21)
# shellcheck disable=SC2086 # the word list is split on purpose
expect_eq "21 words" 2 "$(rc_of "$FIND" --project ABC $words21)"
# shellcheck disable=SC2086
expect_eq "20 words are accepted" 0 "$(rc_of "$FIND" --project ABC $(seq -f 'w%g' 1 20))"
expect_eq "an unknown option" 2 "$(rc_of "$FIND" --project ABC --query x demo)"
reset
rc_of "$FIND" --project abc demo >/dev/null
expect_eq "usage errors make no call" 0 "$(calls)"

# A failing call exits 1 and shows the first lines of its output.
export STUB_FAIL=1
rc=0; err=$(F --project ABC demo 2>&1 >/dev/null) || rc=$?
expect_eq "a failing call exits 1" 1 "$rc"
expect_has "the failure shows the output of the call" "Error in the JQL Query" "$err"
unset STUB_FAIL
if [ -n "$(find "$TMPDIR/aurora-agent" -mindepth 1 -maxdepth 1 2>/dev/null)" ]; then bad "temp files left behind"; else ok "temp files removed"; fi

# === md2jira =============================================================================================
M() { "$MD" "$@"; }
case_md() { # name markdown expected
  local got; got=$(printf '%s\n' "$2" | M)
  expect_eq "md2jira: $1" "$3" "$got"
}
case_md "headings" $'# One\n###### Six' $'h1. One\nh6. Six'
case_md "bold, italic, bold inside italic" $'**b** *i* _j_ *a **b** c*' $'*b* _i_ _j_ _a *b* c_'
case_md "strike" '~~gone~~' '-gone-'
case_md "inline code with a brace" $'run \x60a{b}\x60 now' 'run {{a\{b\}}} now'
case_md "fenced code with a language" $'```python\nx = {1}\n```' $'{code:python}\nx = {1}\n{code}'
case_md "fenced code without a language" $'```\nplain\n```' $'{noformat}\nplain\n{noformat}'
case_md "fenced bash and text are noformat" $'```bash\nls\n```\n```text\nout\n```' $'{noformat}\nls\n{noformat}\n{noformat}\nout\n{noformat}'
case_md "an unknown language is noformat" $'```brainfuck\n+\n```' $'{noformat}\n+\n{noformat}'
case_md "nothing is escaped in code" $'```\n*a* _b_ [c]\n```' $'{noformat}\n*a* _b_ [c]\n{noformat}'
case_md "nested mixed lists" $'- one\n  - two\n    1. three\n1. a\n   - b\n2. c' $'* one\n** two\n**# three\n# a\n#* b\n# c'
case_md "* and + bullets" $'* a\n+ b' $'* a\n* b'
case_md "a link" 'see [the docs](https://example.org/a_b) now' 'see [the docs|https://example.org/a_b] now'
case_md "a bare URL is unchanged" 'go to https://example.org/x_y_z. ok' 'go to https://example.org/x_y_z. ok'
case_md "a table" $'| a | b |\n|---|:--:|\n| 1 | x\\|y |' $'||a||b||\n|1|x\\|y|'
case_md "a one-line quote" '> careful' 'bq. careful'
case_md "a multi-line quote" $'> one\n> two' $'{quote}\none\ntwo\n{quote}'
case_md "a rule" '---' '----'
case_md "blank lines are kept" $'a\n\nb' $'a\n\nb'
case_md "a -b- c is escaped" 'a -b- c' 'a \-b\- c'
case_md "file_name_here is escaped" 'file_name_here' 'file\_name\_here'
case_md "x - y stays readable" 'x - y' 'x - y'
case_md "2*3 stays readable" '2*3' '2*3'
case_md "hyphenated-words stay readable" 'a well-known, non-trivial case' 'a well-known, non-trivial case'
case_md "+ and ^ pairs" 'a +b+ c ^d^ e' 'a \+b\+ c \^d\^ e'
case_md "??citation??" 'a ??b?? c' 'a \?\?b\?\? c'
case_md "a lone _ stays" 'a _ b' 'a _ b'

# Injection: a draft cannot add macros, links, images or headings beyond the Markdown.
case_md "a {html} macro is escaped" 'x {html}<script>1</script>{html}' 'x \{html\}<script>1</script>\{html\}'
case_md "a {code} macro is escaped" '{code}boom{code}' '\{code\}boom\{code\}'
case_md "a {noformat} macro is escaped" '{noformat}' '\{noformat\}'
case_md "{panel} and {color}" '{panel:title=x}{color:red}y{color}{panel}' '\{panel:title=x\}\{color:red\}y\{color\}\{panel\}'
case_md "a Jira link or user mention is escaped" 'ping [~someone] and [Home|http://x]' 'ping \[~someone\] and \[Home\|http://x\]'
case_md "a Jira image is escaped" '!attached.png!' '\!attached.png!'
case_md "a Markdown image does not embed" '![alt](a.png)' '\!alt (a.png)'
case_md "a javascript: link is not a link" '[x](javascript:alert(1))' 'x (javascript:alert(1))'
case_md "heading syntax in a paragraph" 'h1. not a heading' 'h1\. not a heading'
case_md "bq syntax in a paragraph" 'bq. not a quote' 'bq\. not a quote'
case_md "the closing macro inside fenced code cannot break out" $'```\n{code}\n{noformat}\n```' $'{noformat}\n{​code}\n{​noformat}\n{noformat}'
case_md "a macro in a language block cannot break out" $'```sql\nselect 1 {code} x\n```' $'{code:sql}\nselect 1 {​code} x\n{code}'
case_md "a macro in a table cell" $'| a |\n|---|\n| {html} |' $'||a||\n|\\{html\\}|'
case_md "a macro in a link text and a heading" $'# {html}\n[{x}](https://e.org)' $'h1. \\{html\\}\n[\\{x\\}|https://e.org]'
case_md "braces and pipes in a URL are encoded" 'https://e.org/a{b}|c' 'https://e.org/a%7Bb%7D%7Cc'
case_md "a Markdown backslash escape stays literal" 'not \*bold\* here' 'not \*bold\* here'
case_md "backticks keep their content literal" $'\x60{html}\x60 and \x60a|b\x60' '{{\{html\}}} and {{a|b}}'

# --issue-json
printf 'Body with *italic* and \x60x\x60.\n' > "$WORK/draft.md"
j=$(M --issue-json --project ABC --type Task --summary 'Disk full on demo' --label ops --label 'x_y' --priority High < "$WORK/draft.md")
expect_eq "--issue-json with --type: the body" '{"fields":{"project":{"key":"ABC"},"issuetype":{"name":"Task"},"summary":"Disk full on demo","description":"Body with _italic_ and {{x}}.","labels":["ops","x_y"],"priority":{"name":"High"}}}' "$(jq -c . <<<"$j")"
j=$(M --issue-json --project ABC --type-id 10001 --summary 'S' < "$WORK/draft.md")
expect_eq "--issue-json with --type-id: id, no labels, no priority" '{"fields":{"project":{"key":"ABC"},"issuetype":{"id":"10001"},"summary":"S","description":"Body with _italic_ and {{x}}."}}' "$(jq -c . <<<"$j")"
expect_eq "no label is forced" "null" "$(jq -c '.fields.labels' <<<"$j")"
s255=$(printf 'a%.0s' $(seq 1 255))
j=$(M --issue-json --project ABC --type Task --summary "$s255" < "$WORK/draft.md")
expect_eq "a 255-character summary is accepted whole" 255 "$(jq -r '.fields.summary | length' <<<"$j")"
for case in \
  "--project ABC --type Task --summary $(printf 'a%.0s' $(seq 1 256))" \
  "--project abc --type Task --summary S" \
  "--project ABC --summary S" \
  "--project ABC --type Task --type-id 1 --summary S" \
  "--type Task --summary S" \
  "--project ABC --type Task" \
  "--project ABC --type-id x1 --summary S" \
  "--project ABC --type Task --summary S --label a--b=c --label ''" \
  "--project ABC --type Task --summary S --label 'a b'"; do
  # shellcheck disable=SC2086,SC2294
  rc=0; eval "\"$MD\" --issue-json $case" < "$WORK/draft.md" >/dev/null 2>&1 || rc=$?
  expect_eq "--issue-json usage error: ${case:0:60}" 2 "$rc"
done
expect_eq "--issue-json: a summary with a newline" 2 "$(rc_of "$MD" --issue-json --project ABC --type Task --summary $'a\nb' < "$WORK/draft.md")"
expect_eq "--issue-json: a label with a newline" 2 "$(rc_of "$MD" --issue-json --project ABC --type Task --summary S --label $'a\nb' < "$WORK/draft.md")"
expect_eq "--issue-json: an empty priority" 2 "$(rc_of "$MD" --issue-json --project ABC --type Task --summary S --priority '' < "$WORK/draft.md")"
expect_eq "options without --issue-json" 2 "$(rc_of "$MD" --project ABC < "$WORK/draft.md")"
expect_eq "--issue-json and --comment-json together" 2 "$(rc_of "$MD" --issue-json --comment-json < "$WORK/draft.md")"

# --comment-json
j=$(printf '# Note\n\nSee [a](https://e.org) {html}\n' | M --comment-json)
expect_eq "--comment-json" '{"body":"h1. Note\n\nSee [a|https://e.org] \\{html\\}"}' "$(jq -c . <<<"$j")"

# === hygiene =============================================================================================
for s in "$FIND" "$MD"; do
  if grep -nE '\bcurl\b|\bwget\b|requests|urllib|http\.client|import socket|eval\(|exec\(|shell=True|os\.system|TOKEN|Authorization|environ\[' "$s"; then
    bad "$(basename "$s") uses the network, a shell or credentials directly"
  else
    ok "$(basename "$s"): aurora-agent only, no shell, no credentials"
  fi
done

if [ "$fails" -ne 0 ]; then echo "$fails check(s) failed" >&2; exit 1; fi
echo "atlassian skills: all checks passed"
