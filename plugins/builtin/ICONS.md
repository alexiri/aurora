# Built-in package icons

Decision **D-99** (owner, 2026-11-08, P2-CAT-ICONS): the icons of the built-in packages come from
[Simple Icons](https://simpleicons.org) **16.37.0** (npm `simple-icons@16.37.0`, tarball sha256
`cd9faf0e6faa8a8fdd15e9632ab4c86fcae4b986fe552b87673149847f2f671d`), licensed **CC0-1.0**. A product
that is not in Simple Icons takes the icon from the vendor's **official** source (amendment, same
day); only if none exists does it get a neutral **monogram** tile (first letter in the dashboard's
own design-token colours, no brand artwork). Never unofficial redraws or third-party icon sites.

Trademarks stay with their owners. The glyphs identify the product the package connects to; follow
each vendor's brand guidelines (Simple Icons links them from the icon's page on simpleicons.org,
e.g. `https://simpleicons.org/?q=<slug>`) before using one outside the dashboard.

## Rendering

Only the PNGs are committed, never the SVGs. Each is rendered with `rsvg-convert` from the pinned
SVG: 256×256, transparent background, glyph in the brand hex colour, 12 % padding.
`assets/icon-dark.png` is the same glyph for the dark surface (`--surface: #121829`): white where
the brand colour's contrast on that surface is below 3:1, otherwise identical to `icon.png`.
Monograms: rounded tile in `--subtle`/`--line-strong` with the letter in `--primary` (light) and
`--subtle`/`--line-strong`/`--primary` of the dark theme (dark).

## Per package

| Package | Source | Simple Icons slug | Brand hex | SVG sha256 | Dark icon |
|---|---|---|---|---|---|
| `atlassian` | Simple Icons 16.37.0 | `atlassian` | `#0052CC` | `88a88e7a09df2ab7d2e6beefc1c6d8393073fdea18616a4bcc25a30145228edf` | white (2.59:1) |
| `github` | Simple Icons 16.37.0 | `github` | `#181717` | `3bf8cceead820aec50d4ee825a3fd02c5a1cd6665cc9cf4cbf3d9c8861a204bb` | white (1.01:1) |
| `github-app` | Simple Icons 16.37.0 | `github` | `#181717` | `3bf8cceead820aec50d4ee825a3fd02c5a1cd6665cc9cf4cbf3d9c8861a204bb` | white (1.01:1) |
| `gitlab` | Simple Icons 16.37.0 | `gitlab` | `#FC6D26` | `c7c39058bd1b6f9f40334383bd5136bb8c5ba5e6a24200f2d1f18365e2526e28` | same as light (6.19:1) |
| `google-workspace` | Simple Icons 16.37.0 (Google; no Workspace icon) | `google` | `#4285F4` | `d8cf9fb2d5e234e03254f1d6400d8cc8160c309b010d031a64f37084b5a21d01` | same as light (4.96:1) |
| `linear` | Simple Icons 16.37.0 | `linear` | `#5E6AD2` | `90b91b61090f4f9c94f9cb24769642278f03e8eedcfe5680738cf3a186bfc00f` | same as light (3.76:1) |
| `notion` | Simple Icons 16.37.0 | `notion` | `#000000` | `b17d2a2b592a06252efef522d5205f0c7a958f748d40df1011ed081417e42f85` | white (1.19:1) |
| `postgresql` | Simple Icons 16.37.0 | `postgresql` | `#4169E1` | `c8ccadbd88c9312c2ad3734c4861b31d3485ce179ee91d64793143f83cfa745c` | same as light (3.65:1) |
| `sentry` | Simple Icons 16.37.0 | `sentry` | `#362D59` | `8ba32902ffbac503784607374f57df5e3cbcf57a99d0adff03e735575500adc6` | white (1.41:1) |
| `microsoft-365` | monogram "M" | none | n/a | n/a | monogram, dark theme colours |
| `aws-keys` | official vendor source (AWS), see below | none | n/a | see below | AWS's own dark-squid-ink tile |
| `aws-role` | official vendor source (AWS), see below | none | n/a | see below | AWS's own dark-squid-ink tile |
| `indico` | official vendor source (Indico repository), see below | none | n/a | see below | same file as light |

Simple Icons 16.37.0 has no Microsoft 365 / Microsoft, Amazon Web Services or Indico icon (vendors
had them removed). Per the owner's amendment (D-99, 2026-11-08) the fallback is the vendor's official
source, and a monogram only if none exists. If a later Simple Icons release adds one, replace it,
record the new slug, version and sha256 here and bump the package's patch version.

## Official vendor sources (not in Simple Icons)

All fetched on 2026-11-08 from the vendor's own domain; only the rendered PNG is committed.

### `aws-keys`, `aws-role`: AWS Architecture Icons

- Page: <https://aws.amazon.com/architecture/icons/>
- Package: `https://d1.awsstatic.com/onedam/marketing-channels/website/public/shared/architecture-icon-release/Icon-package_07312026.5846e92413caa21490223536cc97f1269e44fa92.zip`
  (sha256 `d2d166c453526471749d520e0db022c459abef759d2946cf2dd1d1c992dc6526`).
- Files used: `Architecture-Group-Icons_07312026/AWS-Cloud-logo_32_Dark.svg` (sha256
  `1c5db2de7a100b1a5e51fcd4bdfd2c5ed76600b3c8b64677d614fa3c402b124c`, white tile with the dark logo)
  for `icon.png` (light theme), and `AWS-Cloud-logo_32.svg` (sha256
  `63d73df1aac59dab323df732f82e196e359fb7b6370c0ca8e51e7b897cd350be`, dark squid-ink tile) for
  `icon-dark.png` (dark theme), so neither theme shows a glaring square. Both are AWS's own variants,
  rendered unaltered (scale only, with `rsvg-convert`) to 256×256. The square tile includes AWS's own background; no extra padding.
- Terms: the AWS Trademark Guidelines <https://aws.amazon.com/trademark-guidelines/> §9: logos must
  not be altered (proportion, colour, font, elements added or removed), must appear by themselves
  with reasonable spacing, and not on a background that hurts readability. §4 limits use to
  materials approved in advance by AWS. **Owner decision (2026-11-08):** keep the official logo
  for the connector listing after reviewing §4. The dashboard must show these tiles unaltered
  (no recolouring, no extra overlay).

### `indico`: Indico project logo

- Source: the official repository <https://github.com/indico/indico>, pinned commit
  `9e1eed53506ffa41966151aa8ca18237bc00cc61`:
  `https://raw.githubusercontent.com/indico/indico/9e1eed53506ffa41966151aa8ca18237bc00cc61/indico/web/static/images/indico_square.png`
  (300×300 official square logo tile; sha256
  `1870afe36ebe769aaf2af0014e6309652b78e143118dfc41bf970fb0c320649c`), scaled to 256×256 with
  `magick`. The tile carries its own background and reads on both surfaces, so `icon-dark.png` is the
  same file. The repository's `*_small.svg` variants are wide wordmarks, not square.
- Terms: the repository is MIT-licensed (© CERN); it states no separate logo or brand terms. Use
  the logo only to identify Indico itself and keep it unaltered.

### `microsoft-365`: monogram (no usable official icon)

Microsoft publishes no downloadable Microsoft 365 product logo. The "Microsoft 365 architecture
templates and icons" download (<https://learn.microsoft.com/en-us/microsoft-365/solutions/architecture-icons-templates>,
`https://go.microsoft.com/fwlink/?linkid=869455`, sha256
`522c4d43cf98a00380b0836f9ec6e06d81fb99758b258c49b6e5f385c05c547a`, 2026-11-08) holds only generic
UI glyphs (no Microsoft 365 or Microsoft mark), and its terms permit use "in architectural diagrams,
training materials, or documentation" only. So the monogram "M" stays. Revisit if Microsoft
publishes a product icon with terms that cover a product listing.

## Built-in connectors

The Mattermost connector (`connectors/mattermost/assets/icon.png`, embedded in its manifest as
`about.icon_png_b64`, P4-MCON-2) uses the same source and rendering: Simple Icons 16.37.0 (tarball
sha256 `cd9faf0e6faa8a8fdd15e9632ab4c86fcae4b986fe552b87673149847f2f671d`), slug `mattermost`, brand
hex `#0058CC`, SVG sha256 `eb0c39d9be0cd4463c484199f672b5e99de37c0ae91ef26678efb95111283df8`, rendered
with `rsvg-convert` at 256×256, transparent background, 12 % padding. Brand guidelines:
<https://www.mattermost.org/brand-guidelines/>. A manifest carries one icon, so there is no dark variant.

## Provenance of the earlier icons

The previous `assets/icon.png` of `gitlab`, `indico`, `postgresql`, `aws-keys` and `aws-role` had
unknown provenance and are replaced; the other eight were 33-byte placeholders.

## Updating

1. Fetch the new release tarball into a scratch directory outside the repo; record its sha256.
2. Render with `rsvg-convert` (treat the SVGs as untrusted data); commit only the PNGs.
3. Update this file (version, sha256 per SVG) and bump the patch version of every changed package.
