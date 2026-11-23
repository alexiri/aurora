#!/usr/bin/env python3
"""Render a week view from Indico events.

Reads a JSON array of {title, start, location, url} objects (from a file
argument or stdin), sorts them by start time, removes duplicates and prints
one compact line per event, grouped by day. Used by the what-is-on skill.
"""
import json
import sys
from collections import defaultdict
from datetime import datetime


def main() -> int:
    raw = sys.argv[1] if len(sys.argv) > 1 else sys.stdin.read()
    try:
        events = json.loads(raw)
    except json.JSONDecodeError as e:
        print(f"not valid JSON: {e}", file=sys.stderr)
        return 1

    seen: set[tuple[str, str]] = set()
    by_day: dict[str, list[tuple[str, str, str]]] = defaultdict(list)
    for ev in events:
        key = (str(ev.get("title", "")), str(ev.get("start", "")))
        if key in seen:
            continue
        seen.add(key)
        start = str(ev.get("start", ""))
        when = start[:16].replace("T", " ")
        location = str(ev.get("location", "") or "")
        url = str(ev.get("url", "") or "")
        line = f"{when}  {key[0]}"
        if location:
            line += f" — {location}"
        if url:
            line += f" ({url})"
        by_day[start[:10]].append((when, line, key[0]))

    if not by_day:
        print("No events.")
        return 0
    for day in sorted(by_day):
        print(day)
        for _, line, _ in sorted(by_day[day]):
            print(f"  {line}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
