# Gateway e2e front doors (TEST-021, tier 2)

CI-only fixtures for `scripts/e2e/gateway.sh` (`FRONTDOOR=1`): strict TLS
terminators that sit **between the gateway and its mock upstream** — the
production topology that escaped FIX-052. Never deployed.

```
client ──proxy──> gateway ──https/h2──> front door ──https/h1──> mock upstream
                                       (this directory)          (:8093, test-owned)
```

| file          | door                        | listen (host net) |
|---------------|-----------------------------|-------------------|
| `nginx.conf`  | nginx 1.29-alpine, :8091    | `127.0.0.2:8091`  |
| `haproxy.cfg` | haproxy 3.2-alpine, :8092   | `127.0.0.2:8092`  |

Both terminate TLS with HTTP/2 enabled (ALPN `h2,http/1.1`), verify the
upstream leg against the mock's certificate, and proxy to `127.0.0.2:8093`
where the test's mock upstream (`tests/e2e-gateway/tests/frontdoor.rs`)
binds. They are strict RFC 9113 peers: an `h2` request that carries both
`:authority` and a `host` header is answered 400 before the upstream sees
it — the failure class FIX-052 removed and this tier keeps out.

## One certificate for both legs

`gateway.sh` generates a 2-day self-signed P-256 leaf for
`fd-mock.e2e.aurora.dev` + `IP:127.0.0.2` into a throwaway
`/tmp/aurora-e2e-frontdoor.*` directory and mounts it read-only into the
doors at `/fd/`:

- the doors verify the **upstream leg** by DNS name against `mock.pem`
  (`proxy_ssl_ca_file` / `ca-file`);
- the gateway verifies the **door leg** by address against the same file,
  handed to the harness as `AURORA_E2E_FRONTDOOR_MOCK_CERT` (the gateway's
  `tls.extra_ca_bundle`); the key never enters a container
  (`AURORA_E2E_FRONTDOOR_MOCK_KEY` stays with the test).

The leaf is generated with `basicConstraints=critical,CA:FALSE`: `openssl
req -x509` defaults to `CA:TRUE`, and rustls/webpki (the gateway's client)
rejects a CA certificate used as a server leaf (`CaUsedAsEndEntity`) even
though OpenSSL/curl tolerate it.

## Environment the script exports

`AURORA_E2E_FRONTDOOR` (nginx), `AURORA_E2E_FRONTDOOR_HAPROXY` (haproxy),
`AURORA_E2E_FRONTDOOR_MOCK` (mock bind address) and
`AURORA_E2E_FRONTDOOR_MOCK_{CERT,KEY}`. Without them the frontdoor tests
print `SKIPPED` and pass — no container is ever started by the test itself.
