# Rotating the signing key and the internal secret

(Phase 2, RFE-007, decision D-51; design in [docs/08 §4](../08-secrets-credentials.md#4-other-key-material-hkdf-purposes).)

`AURORA_SIGNING_KEY` and `AURORA_INTERNAL_SECRET` are **rings**: a comma-separated list of
`<key_id>:<base64>` entries (at most 4) plus the id that is *active*.

| Variable | Holds | Services |
|----------|-------|----------|
| `AURORA_SIGNING_KEY` + `AURORA_SIGNING_ACTIVE_KEY` | ring of 32-byte keys | api |
| `AURORA_INTERNAL_SECRET` + `AURORA_INTERNAL_ACTIVE_KEY` | ring of ≥ 32-byte secrets | api, gateway, hookhost |

- A bare base64 value (no `:`) is still valid: it is a one-entry ring with id `v0`, active, and
  the `*_ACTIVE_KEY` variable may be omitted. Nothing changes until you want to rotate.
- A list **without** its `*_ACTIVE_KEY`, an active id that is not in the list, a duplicate or
  malformed id (`[a-z0-9]{1,16}`) or more than 4 entries makes the service refuse to start,
  naming the variable.
- **Producers use only the active key; receivers accept every key in their ring.** A key id
  outside the ring is refused.
- This does **not** cover the runner session key or the gateway CA (no orchestrated rotation).
  The inline encryption keyring has its own procedure
  ([`aurora-api secrets rekey`](#rotating-the-inline-encryption-keyring-aurora-api-secrets-rekey), docs/08 §3).

What carries a key id:

- action tokens (chat-card buttons) are `v2.<key_id>.<payload>.<tag>`; old `v1.` tokens still
  verify by trying each key,
- OAuth, OIDC and account-link states are prefixed `<key_id>.`; unprefixed ones are tried
  against each key,
- internal calls send `Authorization: Bearer <base64 secret>` and `X-Aurora-Key-Id: <key_id>`
  (the id is not secret). A receiver with no header (old sender) tries every key in its ring.

## Procedure (rolling, no downtime)

The same three steps for either secret. Do not skip the wait between steps: a service that has
not yet received the new key refuses what the others sign with it.

1. **Generate and append.** Print a fresh entry and append it to the ring on **every** service
   that holds the ring (signing key: api; internal secret: api, gateway, hookhost). Leave the
   active id alone. If the variable was a bare value, rewrite it as `v0:<old value>,<new entry>`
   and set `*_ACTIVE_KEY=v0`. Roll the services.

   ```bash
   aurora-api keys generate --kind signing      # prints e.g. k3f9a1c2:Qm9v…=
   aurora-api keys generate --kind internal
   ```

2. **Confirm convergence, then activate.** Every replica must show the new id:

   The internal listeners are closed to every pod but their own peers by NetworkPolicy, and the service images
   have no `curl`, so ask each replica **through `kubectl port-forward`** from where you run `kubectl`
   (a port-forward goes through the API server to the pod and is not subject to NetworkPolicy), once per
   replica (`kubectl get pods` lists them; port-forward to the pod, not the Service, so every replica is asked):

   ```bash
   # the internal secret as stored in the Secret (<fullname>-secrets; `kubectl get secret` lists it)
   SECRET="$(kubectl -n aurora get secret <fullname>-secrets -o jsonpath='{.data.AURORA_INTERNAL_SECRET}' | base64 -d)"
   # A bare value (no ':') is ring entry `v0`: ID=v0, KEY=$SECRET. For a ring `id:key,id2:key2`, use the
   # entry you want to ask about: ID=<id before ':'>, KEY=<the base64 after it>.
   ID=v0; KEY="$SECRET"

   for pod in $(kubectl -n aurora get pods -l app.kubernetes.io/component=gateway -o name); do
     kubectl -n aurora port-forward "$pod" 14103:4103 >/dev/null & pf=$!; sleep 2
     curl -s -H "Authorization: Bearer $KEY" -H "X-Aurora-Key-Id: $ID" http://127.0.0.1:14103/internal/v1/status
     kill $pf
   done
   # hookhost: component=hookhost, port 4104;  api: component=api, port 4111 (both rings)
   ```

   Rehearsed on kind (2026-11-14, a bare-value install): each replica answers
   `{"internal_secret":{"active":"v0","keys":[{"fingerprint":"4a24fe9e","id":"v0"}]},"service":"hookhost"}`
   (the gateway adds its state, the api also the `signing_key` ring); ids and fingerprints only, never values.

   `aurora-api keys status` reads only the environment of the process it runs in: run it with
   `kubectl exec deploy/aurora-api -- /usr/local/bin/aurora-api keys status` for one api replica (the release
   image's path, as in the `secrets rekey` step below; not rehearsed: the kind e2e uses thin dev images). (The first-draft instruction
   to `curl` the internal addresses directly could not be performed from anywhere an operator runs commands.
   If `kubectl port-forward` is not allowed in your cluster, check the ids from each pod's log line at start —
   the services log the ring ids, never values — before activating.)

   The status endpoints and `keys status` print key **ids**, the active id and an 8-hex-char
   fingerprint of each entry. They never print values. Only when every replica of every service
   lists the new id, set `*_ACTIVE_KEY` to it and roll again. Producers now use the new key; the
   old one still verifies what is in flight.

3. **Remove the old key.** After the longest lifetime of anything signed with it, delete its
   entry from the ring (and fix the active id if needed) and roll:

   | Ring | Wait at least |
   |------|---------------|
   | signing key | 25 h (action-token TTL ≤ 24 h plus margin; OAuth/OIDC states live 10 min) |
   | internal secret | 5 min (in-flight internal calls) |

   A removed key stops working at once: states it sealed fail (users retry the sign-in or
   connect flow), chat-card buttons it signed fail (the card is refreshed on the next event),
   internal calls presenting it get 401.

**If a key is compromised**, skip the wait in step 3: append a new key (1), activate it (2) and
remove the compromised one right away. Expect the failures listed above.

## Helm

With a generated Secret, `helm upgrade --set secrets.rotate.signing=true` (and/or
`secrets.rotate.internal=true`) runs the secrets hook once more. It converts a bare value to
`v0:<value>`, **appends one** new entry, writes `*_ACTIVE_KEY` (`v0`) if absent, and never
activates the new key. While a non-active entry is pending, repeated upgrades do not add
another. Then roll the Deployments (`kubectl rollout restart`), continue with step 2 by editing the
Secret (`kubectl edit secret <fullname>-secrets`, values are base64 in `data`), and finish with
step 3. Unset the flag afterwards.

With `secrets.existingSecret`, edit that Secret's keys yourself; the chart only references
`AURORA_SIGNING_ACTIVE_KEY` and `AURORA_INTERNAL_ACTIVE_KEY` as optional keys.

## Rotating the inline encryption keyring (`aurora-api secrets rekey`)

Inline secret values are sealed under `AURORA_ENCRYPTION_KEYS` (comma-separated
`<key_id>:<base64 32 bytes>`; ids `[a-z0-9]{1,16}`), and `AURORA_ENCRYPTION_ACTIVE_KEY` names the
key used for new seals ([docs/08 §3](../08-secrets-credentials.md)). Both the api and the
gateway read it. Every key in the keyring still opens old values; `secrets rekey` re-seals the
old ones under the active key so the old key can be retired.

1. **Generate** a key and an id: `head -c32 /dev/urandom | base64`, id e.g. `k2`.
2. **Add it, keep the old one active.** Append `,k2:<base64>` to `AURORA_ENCRYPTION_KEYS` for the
   api **and** the gateway, leave `AURORA_ENCRYPTION_ACTIVE_KEY` as it is, and roll out both. Until
   every instance knows `k2` no instance may seal under it. (Helm: the chart reads these two keys
   from the Secret named by `secrets.existingSecret` or the generated one; edit them there with
   `kubectl edit secret`, then `kubectl rollout restart`. `secrets.rotate.*` covers only the
   signing key and the internal secret.)
3. **Activate.** Set `AURORA_ENCRYPTION_ACTIVE_KEY=k2` and roll out the api and the gateway.
   New values, and every token refresh, now seal under `k2`.
4. **Re-seal.** Preview, then run it where the api runs:

   ```sh
   kubectl -n aurora exec deploy/aurora-api -- /usr/local/bin/aurora-api secrets rekey --dry-run
   kubectl -n aurora exec deploy/aurora-api -- /usr/local/bin/aurora-api secrets rekey
   ```

   It visits every managed inline value (all eight purposes, or one with `--purpose`) whose
   envelope uses a key other than the active one, and re-seals it under the active key with
   the guarantees of [`secrets migrate`](vault.md#4-migrating-values): per row a
   compare-and-swap update in one transaction, so a row that changed meanwhile is skipped, never
   overwritten; a row that fails its context (purpose, owner, org) or whose key is missing
   fails alone and keeps its old value; values never reach the log or the output. The output is
   one line per purpose (`moved`, `skipped`, `failed`, `already in target`) and the exit status
   is non-zero if any row failed. Values held in Vault and reference secrets are untouched.
5. **Check.** Run `secrets rekey --dry-run` again: every purpose must say `would move 0`. Run it
   once more after any rollout that was still in progress during step 4 (an instance on the old
   active key may have sealed a value meanwhile).
6. **Retire the old key** only now: remove `k1` from `AURORA_ENCRYPTION_KEYS` and roll out. Keep
   it in your backup of the keyring as long as you keep database backups taken before the rekey:
   restoring such a backup needs the key that sealed its values ([backups.md](backups.md)).

## Backups

The whole ring (every entry still in use) belongs in the same backup as the encryption keyring
([backups.md](backups.md)). A restored deployment needs every key that signed a still-valid
artifact.
