#!/usr/bin/env python3
"""FIX-255 (D-491): `#[sqlx::test]` is banned in every Rust file of the repository.

The attribute reads `DATABASE_URL` and `.env` itself, past `aurora-testkit`'s URL rules: without a
URL it panics instead of using the shared test server, and with a `.env` naming port 55432 it runs
the test against the owner's dev database. Tests take their database from the test kit:
`let (_db, pool) = aurora_testkit::TestDb::with_pool().await;` (or `unmigrated_with_pool()`).

Also refused, because each one brings the attribute in under another name: importing `test` from
sqlx (alone or in a brace list), `use sqlx::*`, and renaming the crate (`use sqlx as …`).
Comments are ignored. Usage: no-sqlx-test.py [--self-test]
"""

import re
import subprocess
import sys
from pathlib import Path

CRATE = r"\bsqlx(?:_macros)?\b"
# `sqlx::test` as a path (attribute, cfg_attr, import); not `sqlx::testing` or `sqlx::test_block_on`.
DIRECT = re.compile(CRATE + r"\s*::\s*test\b")
USE = re.compile(r"\buse\s+(?:::\s*)?(sqlx(?:_macros)?\b[^;]*);", re.S)
HELP = "use aurora_testkit::TestDb::with_pool() / unmigrated_with_pool() in a #[tokio::test]"


def blank_comments(text: str) -> str:
    """Replace comments with spaces, keeping line numbers. Strings are left alone."""
    token = re.compile(r'"(?:\\.|[^"\\])*"|//[^\n]*|/\*.*?\*/', re.S)

    def blank(match: re.Match) -> str:
        found = match.group(0)
        if found.startswith('"'):
            return found
        return re.sub(r"[^\n]", " ", found)

    return token.sub(blank, text)


def brace_items(tree: str) -> list[str]:
    """The items directly inside `sqlx::{ … }`."""
    start = tree.find("{")
    if start < 0:
        return []
    items, depth, current = [], 0, ""
    for ch in tree[start + 1 : tree.rfind("}")]:
        depth += (ch == "{") - (ch == "}")
        if ch == "," and depth == 0:
            items.append(current)
            current = ""
        else:
            current += ch
    items.append(current)
    return [item.strip() for item in items]


def findings(text: str) -> list[int]:
    """Line numbers of every banned use."""
    code = blank_comments(text)
    offsets = [m.start() for m in DIRECT.finditer(code)]
    for use in USE.finditer(code):
        tree = use.group(1).strip()
        renamed = re.match(r"sqlx(?:_macros)?\s+as\b", tree)
        glob = re.match(r"sqlx(?:_macros)?\s*::\s*\*", tree)
        braced = re.match(r"sqlx(?:_macros)?\s*::\s*\{", tree) and any(
            re.match(r"(?:test\b|\*$|self\s+as\b)", item) for item in brace_items(tree)
        )
        if renamed or glob or braced:
            offsets.append(use.start())
    return sorted({code.count("\n", 0, at) + 1 for at in offsets})


def self_test() -> None:
    banned = [
        "#[sqlx::test]\nasync fn t() {}",
        '#[sqlx::test(migrator = "aurora_db::MIGRATOR")]\nasync fn t() {}',
        "#[::sqlx::test(migrations = false)]\nasync fn t() {}",
        "#[sqlx :: test]\nasync fn t() {}",
        "#[cfg_attr(unix, sqlx::test)]\nasync fn t() {}",
        "#[sqlx_macros::test]\nasync fn t() {}",
        "use sqlx::test;",
        "use ::sqlx::test as db_test;",
        "use sqlx::{PgPool, test};",
        "pub use sqlx::{\n    PgPool,\n    test as db_test,\n};",
        "use sqlx::*;",
        "use sqlx::{self as db};",
        "use sqlx as db;",
    ]
    allowed = [
        "#[tokio::test]\nasync fn t() {}",
        "// #[sqlx::test] is banned",
        "/// replaces `#[sqlx::test(migrations = false)]`",
        "/* #[sqlx::test] */",
        "use sqlx::{PgPool, Row};",
        "use sqlx::postgres::PgPoolOptions;",
        "use sqlx::testing::TestArgs;",
        "sqlx::test_block_on(async {});",
        'let s = "https://example"; use sqlx::Connection;',
        "use sqlx::{migrate::{Migrator, test_support}};",
    ]
    for text in banned:
        assert findings(text), f"self-test: not caught: {text!r}"
    for text in allowed:
        assert not findings(text), f"self-test: false positive: {text!r}"
    assert findings("a\nb\n#[sqlx::test]\n") == [3], "self-test: line number"


def main() -> int:
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        print("no-sqlx-test: self-test ok")
        return 0
    root = Path(__file__).resolve().parents[2]
    # Tracked and not-yet-tracked files, so a new test file is checked before its first commit.
    files = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "*.rs"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.split("\0")
    files = [f for f in files if f and (root / f).is_file()]
    if not files:
        print("no-sqlx-test: FAIL: no Rust files found", file=sys.stderr)
        return 1
    bad = []
    for name in files:
        text = (root / name).read_text(encoding="utf-8", errors="replace")
        if "sqlx" in text:
            bad += [f"{name}:{line}" for line in findings(text)]
    if bad:
        for where in bad:
            print(f"no-sqlx-test: FAIL: {where}: sqlx::test is banned (FIX-255); {HELP}", file=sys.stderr)
        return 1
    print(f"no-sqlx-test: ok ({len(files)} Rust files)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
