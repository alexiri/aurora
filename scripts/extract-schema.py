#!/usr/bin/env python3
"""Extract the table/column inventory from docs/03-data-model.md.

Writes crates/aurora-db/tests/fixtures/spec_schema.json as
    {"tables": {"<table>": ["col", ...]}}
with tables and columns sorted.

Parsing rules (Python 3 stdlib only):
- A table is a level-3 heading of the form ``### `name` ...`` (any suffix text
  such as ``— *no FKs*`` is ignored). Headings whose table is ``migrations``
  are excluded (managed by sqlx).
- A table only counts if its first markdown row is ``| Column | Type | Notes |``.
  Headings without such a table (e.g. reserved names) are skipped.
- Rows whose first cell is a constraint keyword (unique, partial unique, index,
  check, primary key, foreign key, exclude) are constraints, not columns.
- A Type cell may declare a second column after ``;`` (``int null; signal text null``).
- A first cell may list several comma-separated column names
  (e.g. ``org_id, workspace_id, agent_id``); each name becomes a column.

Usage:
    python3 scripts/extract-schema.py           # regenerate the fixture
    python3 scripts/extract-schema.py --check   # exit 1 if fixture is stale
"""

import json
import os
import re
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPEC_PATH = os.path.join(REPO_ROOT, "docs", "03-data-model.md")
FIXTURE_PATH = os.path.join(
    REPO_ROOT, "crates", "aurora-db", "tests", "fixtures", "spec_schema.json"
)

EXCLUDED_TABLES = {"migrations"}
CONSTRAINT_KEYWORDS = {
    "unique",
    "partial unique",
    "index",
    "check",
    "primary key",
    "foreign key",
    "exclude",
}
HEADING_RE = re.compile(r"^###\s+`([a-z_][a-z0-9_]*)`")
IDENT_RE = re.compile(r"^[a-z_][a-z0-9_]*$")
EXTRA_COL_RE = re.compile(
    r";\s*([a-z_][a-z0-9_]*)\s+(?:uuid|text|timestamptz|int|bigint|bool|jsonb|bytea|citext|inet|real|double)\b"
)


def parse_spec(text):
    tables = {}
    current = None  # table name whose column table is being read
    expect_header = False
    pending_heading = None  # heading seen, waiting for its column table
    for line in text.splitlines():
        m = HEADING_RE.match(line)
        if m:
            pending_heading = m.group(1)
            current = None
            expect_header = False
            continue
        if line.startswith("#"):
            pending_heading = None
            current = None
            continue
        stripped = line.strip()
        if current is None:
            if pending_heading is None:
                continue
            if stripped.startswith("|") and re.match(
                r"^\|\s*Column\s*\|\s*Type\s*\|\s*Notes\s*\|$", stripped
            ):
                expect_header = True
                continue
            if expect_header and re.match(r"^\|\s*-+\s*\|", stripped):
                current = pending_heading
                expect_header = False
                if current not in EXCLUDED_TABLES:
                    tables.setdefault(current, [])
                pending_heading = None
                continue
            if stripped and not stripped.startswith("|") and not expect_header:
                # Prose between heading and table is allowed (e.g. "Partitions...").
                continue
            continue
        # Inside a column table.
        if not stripped.startswith("|"):
            current = None
            continue
        cells = [c.strip() for c in stripped.strip("|").split("|")]
        if not cells or not cells[0]:
            continue
        first = cells[0]
        if first.lower() in CONSTRAINT_KEYWORDS:
            continue
        if re.match(r"^-+$", first):
            continue
        names = [n.strip() for n in first.split(",")]
        # Some rows declare a second column inside the Type cell, e.g.
        # "| exit_code | int null; signal text null |".
        if len(cells) > 1:
            names += EXTRA_COL_RE.findall(cells[1])
        for name in names:
            name = name.strip("`")
            if not IDENT_RE.match(name):
                raise ValueError(
                    f"unparseable column {name!r} in table {current!r}"
                )
            if current not in EXCLUDED_TABLES:
                cols = tables[current]
                if name in cols:
                    raise ValueError(
                        f"duplicate column {name!r} in table {current!r}"
                    )
                cols.append(name)
    return {
        "tables": {
            name: sorted(cols) for name, cols in sorted(tables.items())
        }
    }


def render(data):
    return json.dumps(data, indent=2, sort_keys=False) + "\n"


def main(argv):
    check = "--check" in argv
    with open(SPEC_PATH, encoding="utf-8") as f:
        spec = f.read()
    rendered = render(parse_spec(spec))
    if check:
        try:
            with open(FIXTURE_PATH, encoding="utf-8") as f:
                current = f.read()
        except FileNotFoundError:
            current = None
        if current != rendered:
            sys.stderr.write(
                f"{FIXTURE_PATH} is stale; run scripts/extract-schema.py\n"
            )
            return 1
        print("spec_schema.json is up to date")
        return 0
    os.makedirs(os.path.dirname(FIXTURE_PATH), exist_ok=True)
    with open(FIXTURE_PATH, "w", encoding="utf-8") as f:
        f.write(rendered)
    print(f"wrote {FIXTURE_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
