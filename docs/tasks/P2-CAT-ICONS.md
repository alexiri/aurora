# P2-CAT-ICONS: Vendor icons for the built-in packages from Simple Icons (phase 2)

## Decision (owner, 2026-11-08, D-99)
Built-in package icons come from **Simple Icons** (https://simpleicons.org, CC0-1.0, one-colour SVG glyphs plus the brand hex colour).
- Use the real brand glyph for every package whose product is in Simple Icons.
- If a product isn't in Simple Icons (some vendors have had their icons removed at their request), **don't draw or source a logo elsewhere**. Ship a neutral monogram tile instead: the first letter, our design tokens' colours, no brand artwork. Record that in the report.

## Today
8 of 13 built-ins ship a 33-byte placeholder `assets/icon.png`:
- `atlassian`, `github`, `github-app`, `google-workspace`, `linear`, `microsoft-365`, `notion`, `sentry`.

The other 5 have an icon of **unknown provenance**:
- `gitlab`, `indico`, `postgresql`, `aws-keys`, `aws-role`.

## Scope
1. **Audit all 13 packages.**
   - For each, record the icon's source in `plugins/builtin/ICONS.md`: the Simple Icons slug and version, or "monogram". Include CC0 and a pointer to the vendor's brand guidelines.
   - Replace every icon of unknown provenance with the Simple Icons glyph, or a monogram if the brand isn't there.
2. **Pick the glyph** that names the product:
   - `atlassian` → Atlassian (the package holds Jira and Confluence; icons are per package);
   - `github` / `github-app` → GitHub;
   - `google-workspace` → Google (or Google Workspace, if Simple Icons has it);
   - `microsoft-365` → Microsoft 365 / Microsoft, if present;
   - `aws-*` → Amazon Web Services, if present;
   - `postgresql` → PostgreSQL;
   - `indico` → Indico, if present.
3. **Rendering** with `rsvg-convert` (installed; `inkscape` and `magick` are also available):
   - `assets/icon.png`: 256×256, transparent background, the glyph in the brand hex colour, about 12 % padding.
   - `assets/icon-dark.png`: the same glyph for dark surfaces. Use white, or a lightened brand colour, when the brand colour's contrast on the dark surface token (`web/src/app/globals.css`, dark theme) is below 3:1, e.g. GitHub `#181717` and Notion `#000000`. Otherwise copy the light icon.
   - Both must satisfy `09 §2`: PNG, square, ≥ 64 px, ≤ 256 KiB.
4. **Fetching:**
   - Pin one Simple Icons release and fetch the SVGs from that exact version (the npm package tarball or a jsDelivr URL with the version), into a scratch directory outside the repo.
   - The SVGs are untrusted data: render them with `rsvg-convert` and commit **only the PNGs**, never the SVGs.
   - Record the version and the sha256 of each source SVG in `ICONS.md`.
5. **Versions:** bump the patch version of every package whose assets change (archives are content-addressed; a new icon is new content).

## Constraints
- Only `plugins/builtin/*/assets/*.png`, the `version:` lines of those manifests, and `plugins/builtin/ICONS.md`.
- No manifest, tool or guide changes beyond the version bump.
- Don't touch `web/` (P2-FU-1 adds the dashboard fallback and dark-icon use).
- Decision number: D-99. Record it with the Edit tool.

## Acceptance criteria
- [ ] `aurora plugin validate` passes for every built-in.
- [ ] `bash scripts/plugins/check-builtin.sh` passes for all built-ins.
- [ ] `bash scripts/plugins/build-builtin.sh --build-only` passes.
- [ ] `bash scripts/guards/run-all.sh` passes.
- [ ] A contact sheet of every icon (light and dark, on the light and dark surface colours), as a PNG in the lane's scratch area for the coordinator. Not committed.
