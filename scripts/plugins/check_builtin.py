#!/usr/bin/env python3
"""Checks for every built-in package under plugins/builtin/ (P2-CAT-0, docs/09 §6, §12).

  check_builtin.py <aurora-binary> <package-dir>...

Per package: validate, test, fixture coverage, scopes, token, docs. Link checks are separate
(check_links.py) because they need the network. Needs PyYAML (preinstalled on CI runners).
"""
import fnmatch
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
from urllib.parse import urlsplit

import yaml


# App id -> dangerous requests no tool offers; each needs a `refused <what>: ...` fixture whose
# request selects no tool (FIX-118, PKG-8).
REFUSED = {
    "github": ["repository deletion"],
    "github-ghes": ["repository deletion"],
    "drive": ["file deletion"],
    "calendar": ["calendar clear"],
    "gmail": ["mail deletion"],
    "microsoft-365": ["mail deletion"],
    # (P5-GRAFANA, D-429, SEC-145) credentials, administration, data sources (they hold the
    # backends' credentials), dashboard deletion, plugins, and the Prometheus admin API behind the
    # data source proxy.
    "grafana": ["service-account token creation", "service-account creation", "API key creation",
                "user admin", "organization admin", "datasource creation", "datasource update",
                "datasource deletion", "dashboard deletion", "plugin install",
                "Prometheus admin API"],
}


# (app id, tool id) -> decision: read tools that are a POST and have no `params` classifier because
# the body cannot prove the read. Each is an owner decision, not a pattern: add one only with its D-nnn.
# grafana query_datasources: POST /api/ds/query is how Grafana's own UI reads every data source and
# Grafana grants it to the Viewer role; a write-capable SQL/InfluxDB user is the accepted risk
# (P5-GRAFANA-2, D-462).
POST_READS = {("grafana", "query_datasources"): "D-462"}


def run(aurora, *args, timeout=300):
    p = subprocess.run([aurora, "plugin", *args], capture_output=True, text=True, timeout=timeout)
    return p.returncode, p.stdout, p.stderr


BUILTIN_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "plugins", "builtin")


def other_app_ids(pdir, manifest):
    """App ids of every built-in package except this one (and not ids this package declares)."""
    own = {a["about"]["id"] for a in manifest.get("apps") or []}
    ids = set()
    for d in sorted(os.listdir(BUILTIN_DIR)):
        path = os.path.join(BUILTIN_DIR, d, "plugin.yaml")
        if not os.path.isfile(path) or os.path.samefile(os.path.join(BUILTIN_DIR, d), pdir):
            continue
        with open(path) as f:
            ids.update(a["about"]["id"] for a in (yaml.safe_load(f).get("apps") or []))
    return sorted(ids - own)


def check_package(aurora, pdir):
    # Build the archive once (`aurora plugin build` compiles hooks/ to hooks.wasm for a package
    # with hooks), then run every check on that archive: what is checked is what ships.
    with tempfile.TemporaryDirectory(prefix="check-builtin-") as tmp:
        return check_archive(aurora, pdir, os.path.join(tmp, "package.aurora-plugin"))


def check_archive(aurora, pdir, archive):
    errs = []
    name = os.path.basename(pdir.rstrip("/"))

    def err(msg):
        errs.append(f"{name}: {msg}")

    with open(os.path.join(pdir, "plugin.yaml")) as f:
        manifest = yaml.safe_load(f)
    if manifest["package"]["id"] != name:
        err(f"directory name must equal package id `{manifest['package']['id']}`")
    if manifest["package"]["publisher"] != "aurora":
        err("publisher must be `aurora` (the project key only signs that publisher)")

    hooks = (manifest.get("permissions") or {}).get("hooks") or []

    # 0. build: hooks/ (source crate) is compiled to hooks.wasm; the archive carries hooks.wasm
    # exactly when permissions.hooks is non-empty
    rc, out, serr = run(aurora, "build", pdir, "--out", archive, timeout=1800)
    if rc != 0:
        err(f"aurora plugin build failed:\n{serr or out}")
        return errs
    with tarfile.open(archive) as tf:
        members = tf.getnames()
    if ("hooks.wasm" in members) != bool(hooks):
        err(f"archive {'lacks' if hooks else 'contains'} hooks.wasm but permissions.hooks is "
            f"{'non-empty' if hooks else 'empty'}")
        return errs

    # 1. validate (unsigned allowed: the release pipeline signs). FIX-221, D-465: a skill name is
    # checked against the app ids of every other built-in, so no built-in claims another's prefix.
    others = []
    for o in other_app_ids(pdir, manifest):
        others += ["--other-app-id", o]
    rc, out, _ = run(aurora, "validate", archive, "--allow-unsigned", *others)
    if rc != 0:
        err(f"aurora plugin validate failed:\n{out}")
        return errs

    # 2. test
    rc, out, serr = run(aurora, "test", archive, *others)
    try:
        report = json.loads(out)
    except ValueError:
        err(f"aurora plugin test produced no report: {serr or out}")
        return errs
    fx = report.get("fixtures") or {}
    if rc != 0 or not report.get("ok"):
        bad = [r for r in fx.get("results", []) if r["status"] != "pass"]
        err("aurora plugin test failed: " + "; ".join(f"{r['name']}: {r.get('failures')}" for r in bad[:5]))
        return errs
    if fx.get("skipped"):
        err("fixtures were skipped; every case, hook cases included, must run")

    # fixtures, in the runner's order (files sorted by name, cases in file order)
    cases = []
    tdir = os.path.join(pdir, "tests")
    for fn in sorted(f for f in os.listdir(tdir) if f.endswith(".yaml")):
        with open(os.path.join(tdir, fn)) as f:
            cases += yaml.safe_load(f) or []
    results = fx.get("results", [])
    if len(cases) != len(results):
        err(f"{len(cases)} fixture cases but {len(results)} results")
        return errs
    paired = list(zip(cases, results))

    if os.path.basename(os.path.dirname(pdir.rstrip("/"))) == "builtin":
        check_texts(manifest, pdir, err)  # not the test-only fixture packages
    for app in manifest["apps"]:
        check_app(app, paired, err)
    check_hooks(manifest, hooks, paired, err)
    check_docs(aurora, archive, err)
    return errs


# Page names and placeholders a builtin guide or connect_note may not spell out (P5-UX-13, C-941):
# the guide route fills `{{aurora.*}}` placeholders for the viewer, and "Instances" was renamed
# Self-hosted apps.
REFUSED_TEXT = [
    (re.compile(r"<your Aurora URL>"), "<your Aurora URL> (use {{aurora.callback_url}})"),
    (re.compile(r"<external_url>"), "<external_url> (use {{aurora.callback_url}})"),
    (re.compile(r"Admin, Apps,"), "Admin, Apps, ... (use {{aurora.page:oauth_clients}})"),
    (re.compile(r"\bInstances\b"), "Instances as a page name (it is Self-hosted apps: {{aurora.page:self_hosted}})"),
]


def check_texts(manifest, pdir, err):
    """Texts people read (P5-UX-13, C-939, C-941, C-949): every tool a description of at most 200
    characters, every connect method a label and a description, and no refused literal in the
    guide or a connect_note."""
    for app in manifest["apps"]:
        aid = app["about"]["id"]
        for group in ("read", "write"):
            for t in ((app.get("tools") or {}).get(group) or {}).get("items", []):
                d = (t.get("description") or "").strip()
                if not d:
                    err(f"app {aid}: tool {t['id']} has no description (C-939)")
                elif len(d) > 200:
                    err(f"app {aid}: tool {t['id']} description is {len(d)} characters, at most 200")
        for m in app.get("connect", []):
            for key in ("label", "description"):
                if not (m.get(key) or "").strip():
                    err(f"app {aid}: connect method {m['id']} has no {key} (C-949)")
        note = ((app["about"].get("connect_note") or {}).get("text")) or ""
        for rx, what in REFUSED_TEXT:
            if rx.search(note):
                err(f"app {aid}: connect_note contains {what}")
    with open(os.path.join(pdir, "docs", "setup.md")) as f:
        guide = f.read()
    for rx, what in REFUSED_TEXT:
        if rx.search(guide):
            err(f"docs/setup.md contains {what}")


def hook_steps(c):
    """The `expect` of every call of a hook case (one, or one per `steps` entry)."""
    h = c["hook"]
    return [s.get("expect") or {} for s in h["steps"]] if h.get("steps") else [h.get("expect") or {}]


def check_hooks(manifest, hooks, paired, err):
    """Coverage of hook points (docs/09 §8): every declared point has a passing success case and,
    except approval.summarize, a passing failure case; every egress host is reached by a recorded
    request; egress enforcement is shown by a denied case. Hook cases for undeclared points fail."""
    cases = [(c, r) for c, r in paired if c.get("hook")]
    for c, _ in cases:
        if c["hook"].get("point") not in hooks:
            err(f"hook case `{c['name']}` targets `{c['hook'].get('point')}`, which permissions.hooks does not declare")
    egress = (manifest.get("permissions") or {}).get("egress") or {}
    for point in hooks:
        mine = [(c, r) for c, r in cases if c["hook"].get("point") == point and r["status"] == "pass"]
        if not any(all("error" not in e for e in hook_steps(c)) for c, _ in mine):
            err(f"hook {point}: no passing fixture where the hook succeeds")
        if point != "approval.summarize" and not any(
                any("error" in e for e in hook_steps(c)) for c, _ in mine):
            err(f"hook {point}: no passing fixture where the hook fails (an `expect: {{ error: ... }}` case)")
        hosts = egress.get(point) or []
        static = [h.lower() for h in hosts if h != "{instance}"]
        inst_apps = {a["about"]["id"] for a in manifest["apps"] if a.get("instances")}

        def reaches_instance(c):
            # `{instance}` (D-74) is the connection's registered instance host: reached only by a
            # case of an instance-bound app whose `egress_override` stands for the resolved list
            # and whose recorded request goes to a host of it that no static pattern covers.
            h = c["hook"]
            app_id = h.get("app_id") or (manifest["apps"][0]["about"]["id"] if len(manifest["apps"]) == 1 else None)
            ov = [e.lower() for e in h.get("egress_override") or []]
            return app_id in inst_apps and any(
                (host := url_parts(rec["request"]["url"])[0]) in ov
                and not any(fnmatch.fnmatchcase(host, s) for s in static)
                for rec in h.get("http_cassette") or [])

        for pattern in hosts:
            if pattern == "{instance}":
                if not any(reaches_instance(c) for c, _ in mine):
                    err(f"hook {point}: egress host {pattern} is not reached by any recorded request (http_cassette)")
                continue
            if not any(fnmatch.fnmatchcase(url_parts(rec["request"]["url"])[0], pattern.lower())
                       for c, _ in mine for rec in c["hook"].get("http_cassette") or []):
                err(f"hook {point}: egress host {pattern} is not reached by any recorded request (http_cassette)")
        if hosts and not any(c["hook"].get("egress_override") is not None
                             and any(e.get("error") == "egress-denied" for e in hook_steps(c))
                             for c, _ in mine):
            err(f"hook {point}: no fixture shows egress outside the allow-list being denied "
                "(`egress_override` + `expect: { error: egress-denied }`)")


def url_parts(url):
    u = urlsplit(url)
    return (u.hostname or "").lower(), u.path or "/"


def entry_matches(entry, host, path, instance_hosts=()):
    m = entry["match"]
    if m["host"] == "{instance}":
        # an instance-bound entry serves the app's default_host and a registered instance host
        if host not in instance_hosts:
            return False
    elif not fnmatch.fnmatchcase(host, m["host"].lower()):
        return False
    return path_prefix_matches(m.get("path_prefix"), path)


def path_prefix_matches(pre, path):
    # Whole path segments (FIX-215, 09 §3.4), the rule of the core's
    # `aurora_registry::index::path_prefix_matches`: the path equals the prefix or continues it
    # with `/`; a prefix ending in `/` covers the paths below it. `/rest` matches `/rest/api`,
    # not `/restricted`.
    if pre is None or pre == "":
        return True
    if pre.endswith("/"):
        return path.startswith(pre)
    return path == pre or path.startswith(pre + "/")


def check_app(app, paired, err):
    aid = app["about"]["id"]
    hosts = app.get("hosts", [])
    mine = [(c, r) for c, r in paired if (r.get("observed") or {}).get("app") == aid
            or c.get("expect", {}).get("app") == aid]

    # every host entry: a request that this entry (the most specific one) handles
    def specificity(e):
        return len(e["match"].get("path_prefix") or "")

    default_host = (app.get("instances") or {}).get("default_host")

    for i, e in enumerate(hosts):
        covered = False
        for c, r in mine:
            host, path = url_parts(c["request"]["url"])
            inst = {h for h in (default_host, (c.get("connection") or {}).get("instance_host")) if h}
            cand = [x for x in hosts if entry_matches(x, host, path, inst)]
            if cand and max(cand, key=specificity) is e and r["status"] == "pass":
                covered = True
                break
        if not covered:
            err(f"app {aid}: hosts[{i}] ({e['match']}) is not exercised by any fixture")

    tools = app.get("tools", {})
    approvals = app.get("approvals", {})
    for group in ("read", "write"):
        for t in (tools.get(group) or {}).get("items", []):
            tid = t["id"]
            if not any(r["status"] == "pass" and (r.get("observed") or {}).get("tool") == tid
                       and c.get("expect", {}).get("tool") == tid for c, r in paired):
                err(f"app {aid}: tool {tid} has no positive fixture (expect.tool: {tid})")
            neg = [(c, r) for c, r in paired if c["name"].startswith(f"negative {tid}:")]
            if not neg:
                err(f"app {aid}: tool {tid} has no negative fixture (name it `negative {tid}: ...`)")
            for c, r in neg:
                if (r.get("observed") or {}).get("tool") == tid:
                    err(f"app {aid}: `{c['name']}` selects {tid} but must not")
            if app.get("database"):
                # Database lane (docs/06): every statement is the synthetic `POST /<class>`, and its
                # approval summary is built by the gateway from the statement, not by a template.
                if t["methods"] != ["POST"] or not str(t.get("host", "")).endswith(".db.aurora.internal"):
                    err(f"app {aid}: database tool {tid} must be POST on *.db.aurora.internal")
            elif app.get("own_server"):
                # (P5-CALDAV, D-275) An own-server app's reads are WebDAV (`PROPFIND`/`REPORT`),
                # as C-233(c) exempts databases: the tool's host must be the connection's own.
                if t.get("host") != "{connection}":
                    err(f"app {aid}: own_server tool {tid} must use the host {{connection}}")
                if group == "write" and tid not in approvals:
                    err(f"app {aid}: write tool {tid} has no approvals template (clamped summary)")
            elif group == "read":
                # (P4-GRANTS, Q8, D-124) A `params` classifier (fail closed: the tool matches only
                # when the parameter proves the read) may carry a body method, e.g. AWS Describe*.
                ok = (t.get("git") == "fetch" or t.get("graphql") == "query"
                      or bool(t.get("params"))
                      or set(t["methods"]) <= {"GET", "HEAD"}
                      or ((aid, tid) in POST_READS and t["methods"] == ["POST"]))
                if not ok:
                    err(f"app {aid}: read tool {tid} allows {t['methods']}; reads are GET/HEAD, git fetch, graphql query or a `params` classifier")
            elif tid not in approvals:
                err(f"app {aid}: write tool {tid} has no approvals template (clamped summary)")

    # dangerous untooled endpoints (FIX-118): a case per entry of REFUSED proves the request
    # selects no tool (it is refused under custom grants and `block_untooled`)
    for what in REFUSED.get(app["about"]["id"], []):
        if not any(c["name"].startswith(f"refused {what}:") and r["status"] == "pass"
                   and (r.get("observed") or {}).get("tool") is None
                   and c.get("expect", {}).get("tool") is None for c, r in paired):
            err(f"app {aid}: no passing `refused {what}: ...` case (the request must select no tool)")

    # every summary template: a positive case that asserts the title
    for tid, tpl in approvals.items():
        if not any(c.get("expect", {}).get("tool") == tid and c["expect"].get("summary", {}).get("title")
                   and r["status"] == "pass" for c, r in paired):
            err(f"app {aid}: approvals.{tid} has no fixture asserting expect.summary.title")
        want = {f["label"] for f in tpl.get("fields", [])}
        seen = set()
        for c, _ in paired:
            if c.get("expect", {}).get("tool") == tid:
                seen |= set((c["expect"].get("summary") or {}).get("fields", {}))
        if want - seen:
            err(f"app {aid}: approvals.{tid} fields never asserted by a fixture: {sorted(want - seen)}")

    # scopes: narrowest by default, writes marked
    for m in app.get("connect", []):
        if m.get("kind") != "oauth2":
            continue
        by = {s["scope"]: s for s in m.get("scopes", [])}
        for s in by.values():
            if s.get("access") not in ("read", "write"):
                err(f"app {aid}: scope {s['scope']} must declare access: read|write")
        for d in m.get("default_scopes", []):
            if by.get(d, {}).get("access") != "read":
                err(f"app {aid}: default scope {d} is not a read scope (defaults must be the narrowest)")
        # A provider without OAuth scopes (Notion) declares `scopes: []` explicitly: the key is
        # present and empty. A missing key, or declared scopes with no default, still fails.
        no_scopes_provider = "scopes" in m and m["scopes"] == []
        if not m.get("default_scopes") and not no_scopes_provider:
            err(f"app {aid}: oauth2 method {m['id']} needs explicit default_scopes")

    # token lifecycle
    tok = app.get("token")
    if tok and tok.get("kind") == "oauth2_refresh":
        if not tok.get("reauth_codes"):
            err(f"app {aid}: oauth2_refresh needs reauth_codes")
        if not any(r["status"] == "pass" and (c.get("connection", {}).get("credentials", {}) or {}).get(tok["refresh_field"])
                   and "authorization" in ((c.get("expect", {}).get("upstream") or {}).get("headers") or {})
                   for c, r in mine):
            err(f"app {aid}: no fixture shows the refreshed token being injected (credentials incl. {tok['refresh_field']})")
        # replayed lifecycle (P2-CAT-T): a stale answer, the refresh exchange against the
        # recorded token endpoint and its outcome. Instance-bound apps: on the default host and on
        # a registered instance host.
        def replayed(outcome, want_default):
            for c, r in mine:
                t = (r.get("observed") or {}).get("token") or {}
                if r["status"] != "pass" or t.get("outcome") != outcome or not (c.get("token") or {}).get("http_cassette"):
                    continue
                ih = (c.get("connection") or {}).get("instance_host")
                # no registered instance = the default-host connection (D-86)
                if want_default is None or (ih in (None, default_host)) == want_default:
                    return True
            return False

        kinds = [None] if not default_host else [True, False]
        for outcome in ("refreshed", "reauth_required"):
            for dflt in kinds:
                if not replayed(outcome, dflt):
                    where = "" if dflt is None else (" on the default host" if dflt else " on a registered instance host")
                    err(f"app {aid}: no replayed token case with expect.token.outcome {outcome}{where} "
                        "(`token:` with `http_cassette`, docs/09 §12)")


def check_docs(aurora, pdir, err):
    outs = []
    for _ in range(2):
        rc, out, serr = run(aurora, "docs", pdir, "--format", "md")
        if rc != 0:
            err(f"aurora plugin docs failed: {serr}")
            return
        outs.append(out)
    if outs[0] != outs[1]:
        err("aurora plugin docs output is not deterministic")
    if "## Permissions" not in outs[0] or "| hooks |" not in outs[0]:
        err("rendered docs lack the permissions table")
    rc, out, serr = run(aurora, "docs", pdir, "--format", "html")
    if rc != 0 or "<table>" not in out:
        err(f"aurora plugin docs --format html failed or has no table: {serr}")


def main():
    aurora, dirs = sys.argv[1], sys.argv[2:]
    if not dirs:
        print("no built-in packages found", file=sys.stderr)
        return 1
    errs = []
    for d in dirs:
        e = check_package(aurora, d)
        print(("FAIL " if e else "ok   ") + os.path.basename(d.rstrip("/")))
        errs += e
    for e in errs:
        print("  " + e, file=sys.stderr)
    return 1 if errs else 0


if __name__ == "__main__":
    sys.exit(main())
