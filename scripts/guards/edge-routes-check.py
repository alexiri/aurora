"""Checks one rendered chart (stdin) against the edge rules; used by edge-routes.sh.

usage: edge-routes-check.py <label>

(FIX-102) HTTPRouteRule has no `hostnames`: hostnames exist only at `spec`. A rule that carries one is
pruned or rejected by the API server and leaves a host-less rule that shadows the dashboard, so any
`rules[].hostnames` fails; the previews wildcard must be a spec-level host of its own HTTPRoute, and
the dashboard host's `/` must bind to web.
"""
import sys

import yaml

label = sys.argv[1]
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
routes = []  # (host, path, backend service suffix, port)


def fail(msg):
    sys.exit("edge-routes: FAIL: %s: %s (routes=%s)" % (label, msg, routes))


for d in docs:
    if d["kind"] == "Ingress" and "connector" not in d["metadata"]["name"]:
        for r in d["spec"]["rules"]:
            for p in r["http"]["paths"]:
                b = p["backend"]["service"]
                routes.append((r.get("host", ""), p["path"], b["name"], b["port"]["number"]))
    elif d["kind"] == "HTTPRoute":
        hosts = d["spec"].get("hostnames", [])
        if not hosts:
            fail("HTTPRoute %s has no spec.hostnames" % d["metadata"]["name"])
        for r in d["spec"]["rules"]:
            if "hostnames" in r:
                fail("HTTPRoute %s: rules[].hostnames does not exist in the Gateway API" % d["metadata"]["name"])
            for m in r["matches"]:
                for b in r["backendRefs"]:
                    for h in hosts:
                        routes.append((h, m["path"]["value"], b["name"], b["port"]))
if not routes:
    fail("no routes rendered")
for host, path, svc, port in routes:
    if path.startswith(("/internal", "/metrics")):
        fail("routes " + path)
    if host.startswith("*."):
        # (P4-PRV-2) the previews wildcard serves everything through the relay, nothing else.
        if not (svc.endswith("-relay") and port == 4112):
            fail("wildcard host %s must route to relay:4112 only" % host)
        continue
    if path == "/v1" and not (svc.endswith("-api") and port == 4101):
        fail("/v1 must go to api:4101")
    # (P4-GRP-2) SCIM: /scim (the /scim/v2 plane) is served by the API, only when enabled.
    if path == "/scim" and not (svc.endswith("-api") and port == 4101):
        fail("/scim must go to api:4101")
    if path == "/" and not (svc.endswith("-web") and port == 4100):
        fail("/ must go to web:4100")
    if path == "/connectors" and not (svc.endswith("-connector-host") and port == 4110):
        fail("/connectors must go to connector-host:4110")
if not any(p == "/v1" for _, p, _, _ in routes):
    fail("no /v1 route")
# (FIX-102) the dashboard host's `/` is web, whatever else renders.
if not any(p == "/" and not h.startswith("*.") and s.endswith("-web") for h, p, s, _ in routes):
    fail("no / route to web on the dashboard host")
# (P4-MCON) the platform callbacks of managed connectors are served by the connector host on one port.
if not any(p == "/connectors" for _, p, _, _ in routes):
    fail("no /connectors route (connector host)")
print("edge-routes: %s ok" % label)
