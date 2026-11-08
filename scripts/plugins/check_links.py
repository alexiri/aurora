#!/usr/bin/env python3
"""Resolve the documentation links of built-in packages (network; not part of blocking CI).

  check_links.py <package-dir>...

A link passes on any 2xx/3xx, or a 401/403/429 (the server answered but wants a login, blocks bots
or rate-limits us).
Each URL is tried with HEAD then GET and retried, so a flaky network does not fail on one blip.
"""
import os
import sys
import time
import urllib.error
import urllib.request

import yaml

UA = "Mozilla/5.0 (compatible; aurora-plugin-link-check/1.0)"


def links(manifest):
    out = []
    homepage = manifest["package"].get("homepage")
    if homepage:
        out.append(("package.homepage", homepage))
    for app in manifest["apps"]:
        about = app["about"]
        for k in ("api_docs", "api_spec"):
            if about.get(k):
                out.append((f"{about['id']}.{k}", about[k]))
        note = (about.get("connect_note") or {}).get("url")
        if note:
            out.append((f"{about['id']}.connect_note.url", note))
        for m in app.get("connect", []):
            for f in m.get("fields", []) or []:
                if f.get("help_url"):
                    out.append((f"{about['id']}.{m['id']}.{f['name']}.help_url", f["help_url"]))
    return out


def ok(url):
    last = ""
    for attempt in range(3):
        for method in ("HEAD", "GET"):
            req = urllib.request.Request(url, method=method, headers={"User-Agent": UA})
            try:
                with urllib.request.urlopen(req, timeout=20) as r:
                    if r.status < 400:
                        return True, ""
            except urllib.error.HTTPError as e:
                if e.code in (401, 403, 429):
                    return True, ""
                last = f"HTTP {e.code}"
            except Exception as e:  # noqa: BLE001 - network errors of any kind
                last = type(e).__name__
        time.sleep(2 * (attempt + 1))
    return False, last


def main():
    bad = 0
    for d in sys.argv[1:]:
        with open(os.path.join(d, "plugin.yaml")) as f:
            m = yaml.safe_load(f)
        for what, url in links(m):
            good, why = ok(url)
            print(("ok   " if good else "FAIL ") + f"{os.path.basename(d.rstrip('/'))} {what} {url} {why}")
            bad += not good
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
