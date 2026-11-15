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


TYPE_RE = re.compile(r"^`?([a-z][a-z0-9]*(?:\[\])?)`?(?=\s|$|;|,)")


def column_type(cell):
    """(FIX-101) The declared type and nullability of one Type cell (or one `;`-separated part).

    Type is the first word (``uuid``, ``text[]``, ``timestamptz`` …); nullable is True for ``null``, False for
    ``not null`` or ``pk``, None when the cell says neither (the guard then compares the type only).
    """
    cell = cell.strip()
    m = TYPE_RE.match(cell)
    typ = m.group(1) if m else None
    low = " " + cell.lower()
    if " not null" in low or re.search(r"\bpk\b", low):
        nullable = False
    elif re.search(r"\bnull\b", low):
        nullable = True
    else:
        nullable = None
    return typ, nullable


def parse_spec(text):
    tables = {}
    columns = {}
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
        extra = EXTRA_COL_RE.findall(cells[1]) if len(cells) > 1 else []
        names_main = list(names)
        names += extra
        # (FIX-101) types: the first part of the Type cell types the listed names, each later `;` part
        # `<name> <type> ...` types the extra column it declares.
        col_types = {}
        if len(cells) > 1:
            parts = cells[1].split(";")
            # `a, b | t1 null, t2 null …`: one comma-separated type per listed name; otherwise every
            # listed name takes the whole first part.
            per_name = [s.strip() for s in parts[0].split(",")]
            for i, n in enumerate(names_main):
                col_types[n.strip("`")] = column_type(
                    per_name[i] if len(per_name) == len(names_main) else parts[0]
                )
            for part in parts[1:]:
                words = part.strip().split(None, 1)
                if len(words) == 2 and words[0] in extra:
                    col_types[words[0]] = column_type(words[1])
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
                if name in col_types:
                    typ, nullable = col_types[name]
                    # "<type> not null" | "<type> null" | "<type>" (nullability not stated)
                    columns.setdefault(current, {})[name] = (
                        f"{typ} {'null' if nullable else 'not null'}"
                        if nullable is not None
                        else f"{typ}"
                    )
    return {
        "tables": {
            name: sorted(cols) for name, cols in sorted(tables.items())
        },
        # (FIX-101) "<type> [not] null" per column (nullability omitted when 03 states neither); the schema
        # guard compares type and nullability with the migrated catalog. Constraints, indexes, defaults,
        # checks and foreign keys are NOT compared (docs/18 §Schema guard).
        "columns": {
            t: {c: v for c, v in sorted(cs.items())}
            for t, cs in sorted(columns.items())
        },
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
