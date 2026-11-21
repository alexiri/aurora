# P6-ENCODED-PATHS: address a GitLab project (and a file in a folder) by its path — **proposal, phase 6**

> **Status: proposal (2026-11-21).** Owner, 2026-11-21, answering the FIX-216 finding: "numeric IDs ok
> for now, write brief to improve this later" (D-451). Nothing here is decided. No C-, SEC- or migration
> numbers are taken yet; the spec batch takes them from PROGRESS's next-free note. Open questions are
> `Q-P6-ENC-n` at the end; the recommendations are the coordinator's. **Security-sensitive: an Opus spec
> pass and an Orchestrator security review before merge.** Don't start until the owner says so.

## Goal

GitLab's REST API addresses a project or group by its numeric id **or** by its full path with each `/`
written `%2F` (`/api/v4/projects/group%2Fsub%2Fproject`), and a repository file by its path encoded the
same way (`/repository/files/src%2Fmain.rs/raw`). People and agents know projects by path, not by id.

Today the gateway refuses every encoded slash before matching (`06` step 3, 400 `ambiguous_path`), so
that the matcher and the upstream can never disagree about where a segment ends. FIX-216 found that the
gitlab package's encoded-path tools never worked through the gateway; gitlab 1.4.2 addresses projects by
numeric id only, and an agent reads a file in a folder with git over HTTPS (D-451).

This brief lets a package declare, per tool path parameter, that **one segment may contain encoded
slashes**, without loosening anything for any other request.

## Today

| Where | What |
|---|---|
| `services/aurora-gateway/src/hygiene.rs` `normalize_path` | refuses `%2F`, `%5C`, `%00`, `\`, NUL and `.`/`..` segments; decodes unreserved `%XX`; collapses `//` |
| `06` step 3 | the rule, applied to every request before app identification (step 5) |
| `09` §tools | a tool path pattern's `*` matches exactly one segment; patterns are exact (no `**`) for proxies |
| `check_snapshot.rs` (FIX-216) | the offline runner applies the same function, so fixtures see production's answer |
| `plugins/builtin/gitlab` 1.4.2 | `$P` is the numeric id; `refused: ambiguous_path` fixtures for `group%2Fdemo` and `src%2Fmain.rs` |

## Proposal

1. **Declaration.** A tool's path parameter may carry `encoded_slash: true` in `operations.json` (and the
   tool's path pattern marks that segment, e.g. `/api/v4/projects/{project}/merge_requests`). Only
   built-in packages and packages an instance admin installs may declare it; the package linter refuses it
   on a `**` pattern, on the first segment, on a host-entry `path_prefix`, and on more than two segments of
   one pattern.
2. **Step 3 stays strict for everything else.** For a request whose host has no package that declares
   `encoded_slash`, nothing changes. For such a host, step 3 keeps `%2F` **inside a segment, still encoded**
   (never decoded to `/`, never split), and refuses as today: `%5C`, NUL, `\`, `%252F` (a double-encoded
   slash), and any segment whose **decoded** parts include `.`, `..` or an empty part
   (`a%2F..%2Fb`, `%2Fa`, `a%2F%2Fb`).
3. **Step 5 decides.** The request is accepted only if the matched tool's pattern puts **every** segment
   that contains `%2F` at a position whose parameter declares `encoded_slash`. Any other `%2F` (an
   untooled request, a non-declared position, a request under `block_untooled`) is refused with
   `ambiguous_path`, as today. Credential injection is decided after this, so a token is never injected on
   a request that step 5 refuses.
4. **Forwarding.** The upstream receives the segment byte-for-byte as matched (`%2F` stays `%2F`). The
   approval card, the request log and summaries show the decoded value (`group/sub/project`) as text.
5. **The offline runner** (`check_snapshot.rs`) and `check_builtin.py` apply the same rule, so a fixture
   expects either a tool or `refused: ambiguous_path` exactly as production answers.
6. **gitlab 1.5.0**: `project` on every project tool, `id` on group tools and `file_path` on the repository
   file tools declare `encoded_slash`; fixtures for a full path, a nested group, a file in a folder, and the
   refusals above (dot parts, double encoding, a `%2F` in a non-declared position, an untooled path with
   `%2F`). `api-conventions.md` and the app instructions go back to "numeric id or full path".

## Security notes (for the spec pass)

- **The threat** step 3 guards against is a matcher/upstream disagreement: the gateway matches one path and
  the upstream routes another (a reverse proxy in front of GitLab that decodes `%2F`, `AllowEncodedSlashes`
  on Apache). The declaration is per parameter of a tool whose upstream documents encoded paths, so the
  package author vouches for that upstream; the risk remains for a self-managed instance behind a proxy
  that decodes. **Q-P6-ENC-2** asks whether that is acceptable or needs a per-connection switch.
- An encoded segment never widens a grant: the tool, its approval and its summary are decided on the
  declared pattern, and a dot part anywhere in the decoded segment is refused.
- A SEC row (taken by the spec batch) with tests that fail without each rule: no `%2F` accepted on a host
  without a declaration; none outside a declared position; none under `block_untooled` without a tool;
  dot parts and double encoding refused; injection never happens on a refused request; upstream receives
  the bytes the matcher saw.

## Work (when started)

`06` step 3/5, `09` (declaration, linter rules), `hygiene.rs` (a variant that keeps declared segments),
the step-5 check in the pipeline, `check_snapshot.rs`, `check_builtin.py`, the registry validator, gitlab
1.5.0 with fixtures, the SEC row in `docs/tests/security-coverage.md`, gateway ITs.

## Open questions

- **Q-P6-ENC-1**: declare per tool parameter (recommended: the narrowest; a package can't open a whole
  host) or per app?
- **Q-P6-ENC-2**: a self-managed GitLab behind a proxy that decodes `%2F` would route differently from
  the gateway's match. Accept (recommended: GitLab's own docs require encoded paths to reach it intact,
  and a decoding proxy breaks GitLab itself), or add a per-connection "use numeric ids only" switch?
- **Q-P6-ENC-3**: should the gitlab package also resolve a path to its id first (`GET /projects/<path>`)
  and then use the id, instead of forwarding encoded paths for writes? Recommended: no, one rule for reads
  and writes keeps approvals simple.
