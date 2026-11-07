# Aurora brand assets

These are the logo files for the brand identity (decision D-68,
`14 §Visual design §Brand`).

## Masters

`aurora-logo-final-light.svg` and `aurora-logo-final-dark.svg` are the owner's
final logos (Inkscape SVG, 400 × 250). They are the **source of truth**. Each
contains:

| Part | Light | Dark |
|---|---|---|
| Mark: deep-teal under-leg (`#117673` → `#066B5E`) and mint over-band (`#3ED0B7` → `#94EBD9`), in a `0 0 280 152` box placed at (100, 10), 200 wide | same | same |
| Star | `#0E1525` | `#E8ECF3` |
| Wordmark "AURORA": Chromatic Void Sans, **outlined** | `#0E1525` | `#E8ECF3` |
| Tagline "BUILD · RUN · COLLABORATE": Space Grotesk Bold, **outlined** | `#5B6576` | `#B4BDCC` |

The text is outlines, so no font is needed to display any file here.

## Generated variants

Every other SVG in this folder is generated from the masters. The mark, star,
wordmark and tagline paths are copied verbatim, and only placement and colour
change. To change the logo, edit a master and regenerate; never edit a variant
by hand.

| File | Use |
|---|---|
| `aurora-lockup.svg` / `aurora-lockup-dark.svg` | Stacked mark + wordmark + tagline: the masters without editor metadata. For the README, docs and splash screens |
| `aurora-lockup-horizontal.svg` / `-dark.svg` | Mark + wordmark in a row (236 × 64): the dashboard sidebar, the auth brand panel, headers, email |
| `aurora-wordmark.svg` / `aurora-wordmark-dark.svg` | The outlined wordmark alone (252 × 42) |
| `aurora-mark.svg` / `aurora-mark-dark.svg` | The mark alone (280 × 152), with an ink or `#E8ECF3` star |
| `aurora-mark-mono-ink.svg` / `aurora-mark-mono-white.svg` | Single colour, below 24 px. A masked gap along the fold keeps the twist readable |
| `aurora-favicon.svg` | Favicon (64 × 64); the star follows `prefers-color-scheme` |
| `app-icon-light.svg` / `app-icon-dark.svg` | App and PWA icons, `apple-icon` (rasterise to committed PNGs) |
| `app-icon-mono-light.svg` / `app-icon-mono-dark.svg` | Single-colour app icons (pinned tabs, monochrome launchers) |

## Rules

- **Twist.** The mark is one ribbon: the teal leg passes under the mint band.
  Keep the fold edge, the pointed inner apex and the flared feet with flat
  cuts. Never redraw it as two strokes.
- **One mark, both themes.** Only the star, wordmark and tagline colours
  change between light and dark.
- **Wordmark.** Never retype "AURORA" in a font; use the outlines. The tagline
  belongs only to the stacked lockup and the auth brand panel, never the app
  chrome.
- **Clear space.** At least the star's height on every side.
- **Minimum size.** 16 px wide for the mono mark, 24 px for the colour mark,
  96 px wide for the horizontal lockup.

## Fonts (`fonts/`)

These are the source fonts used to letter the masters. They are kept so the
wordmark and tagline can be re-outlined if the logo is edited. **The dashboard
does not load them:** the logo files already contain the outlines. Body and UI
fonts are self-hosted from npm packages instead (I-40).

| File | Used for | Licence |
|---|---|---|
| `chromatic-void-sans-regular.ttf` | Wordmark "AURORA" | **Not recorded in the font** (no copyright or licence fields; vendor field `mixfont.com`). Confirm the terms before redistributing the font or using the logo commercially |
| `SpaceGrotesk-{Light,Regular,Medium,SemiBold,Bold}.ttf` | Tagline (Bold) | SIL Open Font License 1.1, see `SpaceGrotesk-OFL.txt` |

## In the dashboard

- Render the logo with the `AuroraMark` and `AuroraLogo` components: inline
  SVG with the path data copied from these files, so the colours follow the
  theme and gradient ids stay unique (`useId`). Don't use `<img>` inside the
  app.
- Copy `aurora-favicon.svg` to `web/src/app/icon.svg`.
- Generate the PNG app icons from `app-icon-*.svg`, commit them, and serve them
  from the dashboard's origin.
