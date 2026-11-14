"""FIX-119 guard: the route catalogue of docs/05 against web/openapi.json, and the invariant table of docs/17.

usage: docs-routes.py [--self-test]

1. Every `| METHOD `/path` ...` row of docs/05 §Route catalogue (except /internal/*) must be
   - an operation of web/openapi.json (placeholder names ignored), or
   - listed in the "Outside the OpenAPI document" table of docs/05, or
   - tagged "(phase 5" / "removed in phase" / "phase 0 only" in the row, or sit under a heading that is
     (phase 5 …) or names a machine plane ("Planes", "Relay plane").
2. docs/17-security.md: the invariant table is one block — a blank line between two `| SEC-` rows ends the table,
   so the rows after it render as text.
"""
import json
import re
import sys

METHODS = ("GET", "POST", "PUT", "PATCH", "DELETE")


def norm(path):
    return re.sub(r"\{[^}]*\}", "{}", path.split("?")[0].rstrip("/"))


def route_problems(doc_text, spec_paths):
    have = {(m.upper(), norm(p)) for p, v in spec_paths.items() for m in v if m.upper() in METHODS}
    outside = set()
    m = re.search(r"\*\*Outside the OpenAPI document.*?\n\n\|.*?\n\n", doc_text, re.S)
    if m:
        for line in m.group(0).splitlines():
            r = re.match(r"^\|\s*(GET|POST|PUT|PATCH|DELETE)\s+`(/[^`]*)`", line)
            if r:
                outside.add((r.group(1), norm(r.group(2))))
    problems = []
    heading = ""
    in_catalogue = False
    for line in doc_text.splitlines():
        if line.startswith("## "):
            in_catalogue = line.strip() == "## Route catalogue"
        if line.startswith("### "):
            heading = line
        if not in_catalogue:
            continue
        r = re.match(r"^\|\s*(GET|POST|PUT|PATCH|DELETE)\s+`(/[^`]*)`", line)
        if not r:
            continue
        method, path = r.group(1), norm(r.group(2))
        if path.startswith("/internal"):
            continue
        if (method, path) in have or (method, path) in outside:
            continue
        tagged = re.search(r"\(phase 5|removed in phase|phase 0 only", line) or re.search(
            r"\(phase 5|Planes|Relay plane", heading
        )
        if tagged:
            continue
        problems.append("%s %s is in the docs/05 catalogue but not in web/openapi.json, not in the 'outside' table, and not phase-tagged" % (method, r.group(2)))
    return problems


def table_problems(text):
    problems = []
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if not line.startswith("| SEC-"):
            continue
        j = i - 1
        blanks = 0
        while j >= 0 and lines[j].strip() == "":
            blanks += 1
            j -= 1
        if blanks and j >= 0 and lines[j].startswith("|"):
            problems.append("docs/17: a blank line before %s ends the invariant table" % line.split("|")[1].strip())
    return problems


def self_test():
    spec = {"/v1/a/{x}": {"get": {}}}
    doc = (
        "## Route catalogue\n### Things\n| GET `/v1/a/{id}` | S |\n| GET `/v1/b` | S | unbuilt |\n"
        "| GET `/v1/c` | S | (phase 5, D-138) not built |\n"
    )
    got = route_problems(doc, spec)
    assert len(got) == 1 and "/v1/b" in got[0], got
    assert table_problems("| SEC-01 | a |\n\n| SEC-02 | b |\n"), "a split table must be reported"
    assert not table_problems("| SEC-01 | a |\n| SEC-02 | b |\n"), "a contiguous table must pass"


def main():
    if "--self-test" in sys.argv:
        self_test()
        print("docs-routes: self-test ok")
        return 0
    spec = json.load(open("web/openapi.json"))["paths"]
    doc = open("docs/05-control-plane-api.md", encoding="utf-8").read()
    sec = open("docs/17-security.md", encoding="utf-8").read()
    problems = route_problems(doc, spec) + table_problems(sec)
    if problems:
        for p in problems:
            print("docs-routes: FAIL: " + p, file=sys.stderr)
        return 1
    n = len(re.findall(r"^\|\s*(?:GET|POST|PUT|PATCH|DELETE)\s+`/", doc, re.M))
    print("docs-routes: ok (%d catalogue rows checked; docs/17 invariant table is contiguous)" % n)
    return 0


if __name__ == "__main__":
    sys.exit(main())
