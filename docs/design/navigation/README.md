# Navigation review — the design package (read this first)

On 2026-11-27 the owner chose two proposals of the Navigation review and accepted every proposed decision (**D-507**,
**D-508**):

- **nav-2a — agent settings as tabs, on every agent page**: one fixed agent header with the tabs Chat · Automations ·
  Computer · Settings; a quieter second row only under Computer and Settings; every section at full width. Built by
  **P5-NAV-1**, checked by **P5-NAV-4**.
- **nav-3 — places**: the rail always shows one place (a workspace, its organization, Administration, Your account), its
  top says which (the place card, nav-4) with a way back, the switcher becomes a place picker, and **Manage <Org> is a
  place whose menu replaces the rail** (nav-1, grouped as nav-3b), on desktop and phone (nav-5, nav-3d). Built by
  **P5-NAV-2** (places, Manage, the place card) and **P5-NAV-3** (the picker, the rail-foot entry, the phone), checked by
  **P5-NAV-5**.

nav-2b and nav-2c were **not** chosen. This folder is the offline copy of the review (the owner's artifact
<https://claude.ai/artifact/QQx4Ds97jQHpM6VuRUtqN6> is private; workers cannot and need not open it).

## What is here

| Path | What it is | How to use it |
|---|---|---|
| `review.md` | Findings N-1..N-10, every proposal's text, the spec tables (who sees what in the picker; where today's Manage entries go), the owner's decisions | Read the parts your brief names, in full |
| `after/<key>.png`, `after/<key>-light.png` | The mockup, dark and light, 1200 px wide (components 680 px) | **Look at it** (the Read tool shows PNGs) before coding and before handing over |
| `after/<key>.html` | The mockup as live HTML (uses `../kit.css`) | Open in a browser (`?light` for light) for exact words and spacing |
| `before/<shot>.png` | Today's Aurora, 1256 px desktop and 390 px phone | What is replaced |
| `kit.css` | The mockup kit | Only to render the HTML; never copy it into the app |

| Key | What it shows | Brief |
|---|---|---|
| nav-2a | Settings › Memory with the header tabs and the section pills | P5-NAV-1 |
| nav-2a-chat, nav-2a-automations, nav-2a-computer | The same header on Chat, Automations, Computer › Files | P5-NAV-1 |
| nav-2a-phone | Chat, Computer › Files, Settings › Instructions at 390 px | P5-NAV-1 |
| nav-1, nav-3b | Manage Acme as a place: the rail is the organization's menu (nav-3b is the grouped, final version; where they differ, **nav-3b wins**) | P5-NAV-2 |
| nav-4 | The place card at the top of the rail, in every place | P5-NAV-2 |
| nav-3c | How the places connect | P5-NAV-2, P5-NAV-3 |
| nav-3, nav-3a | The place picker (nav-3a, for three people, is the final version; **nav-3a wins**) and the rail-foot Manage entry | P5-NAV-3 |
| nav-3d, nav-5 | Places on a phone: place card in the top bar, the picker as a sheet, the drawer with the place's menu; the agent tabs on a phone | P5-NAV-3 (nav-5's middle frame: P5-NAV-1) |

## How to read a mockup (binding)

1. **Precedence:** your brief → the owner's decisions (D-507, D-508) → the mockup (the later, more detailed one where two
   overlap, as listed above) → the proposal text in `review.md`.
2. **Sample data is illustrative** (Acme, Globex, Daneel, counts, dates, file names, the 14-workspace Carol). Interface
   words (entry names, group labels, captions such as "Settings for everyone on this Aurora", back lines) are exact.
3. **Yellow notes** (`.ann`) are commentary, not UI.
4. **Build with the app's tokens and shared primitives** (`docs/14-web-dashboard.md` §Visual design): `Tabs`, `Menu`,
   `Badge`, `Dialog` (sheet variant on a phone), the rail components in `app-shell.tsx`. A pattern with no primitive yet
   (the quiet sub-page pill row, the place card) is added once to `web/src/components/ui/` or the shell, and used
   everywhere.
5. **Both themes, 1256 px and 390 px, no horizontal scroll at 390 px.**
6. **Compare before you hand over:** capture your screens at 1256 px (dark and light) and 390 px beside the mockup, and
   list every remaining difference in the completion report with the reason. The review briefs send back what is not
   explained.
