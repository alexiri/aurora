# P5-POLISH-1: Dialogs, CSS layers, focus and menus (owner, 2026-11-19)

## Goal

Fix the two bugs the owner hit in the finished redesign, at their root, so every screen that uses the
shared code is repaired at once. Settle the dialog, focus and menu rules the later P5-POLISH briefs
build on:

1. **Every dialog hides its first line.** The title row in `components/ui/dialog.tsx` is
   `sticky top-0 -mx-6 -mt-6` inside a `p-6` scroll container. A sticky box is offset from the padding
   edge, so the row is drawn 24 px below its place in the flow and covers the top 24 px of the body.
   UX review 4 measured it: title row 388–456 px, body starting at 436 px. The owner saw the connect
   journey's step labels (APP / SIGN IN / DONE) cut in half. The bug also hides the whole confirmation
   sentence on the journey's Done step, and half of the only button in "Daneel and Team calendar".
2. **Links styled as primary buttons are blank.** `app/globals.css:190` has an unlayered
   `a { color: inherit; }`. Unlayered CSS beats Tailwind v4's layered utilities, so `text-on-primary`
   loses and the text takes the surrounding ink colour, which is the primary background. "Open chat" on
   the journey's Done step is a light rectangle in dark theme and a dark rectangle in light theme, with
   no readable text.
3. **Focus and menus look broken:**
   - the chat composer always shows a double teal ring;
   - open menus get a thick teal frame;
   - opening any menu or dialog logs a CSP inline-style violation in the console.

## Linked requirements

- **Design reference (D-380).** Proposals **A1**, **A2**, **F3** and the dialog part of **B3** of “Aurora
  UX fixes”, <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW> (use the anchors `#a1`, `#a2`, `#f3`,
  `#b3`).
  - You cannot open artifacts. Read `/home/alex/orca/workspaces/aurora/design-ux-fixes/README.txt`,
    then:
    - `after/a1.png`, `after/a2.png`, `after/f3.png`, `after/b3.png`: the mockups;
    - `before/a1.png`, `before/a2.png`, `before/f3a.png`, `before/f3b.png`, `before/b3.png`: today's
      screens;
    - `after/src/<id>.html`: the exact markup and tokens of each mockup.
  - The review that found the problems: <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (copy at
    `review/index.html`; screenshots 250, 252, 121, 504, 605, 200, 060 under `review/img/`).
  - Yellow dashed labels in a mockup are commentary, not design. Build with the app's tokens and
    primitives; never copy the mockup's HTML or class names.
- **D-393** (this brief's rules). `14 §Visual design` › "UX fixes (phase 5, P5-POLISH)": Dialogs and
  Focus bullets.
- FIX-149 / D-272 introduced the sticky title row so the close icon stays reachable. Keep that goal:
  the header row never scrolls.

## Wave and order

- **First, in parallel with P5-POLISH-3 (Rust) and P5-POLISH-8 (navigation).** No dependency.
- Every later web brief (P5-POLISH-2, -5, -6, -7, -9, -10) builds on its Dialog and focus rules.
- Lane A: **P5-POLISH-1** → P5-POLISH-2 → P5-POLISH-6 → P5-POLISH-7 → P5-POLISH-9.

## Context

All paths are under `web/src/`.

- **Dialog:** `components/ui/dialog.tsx`.
  - Sticky header at :49–50. Close button at :61–69. Children wrapper `<div className="mt-1">` at :71.
  - Exports at :10–28: `Dialog`, `DialogTrigger`, `DialogClose`, `DialogSize`
    (`sm|md|lg|xl|2xl`, :19) and `DialogContent`. `DialogContent` takes `title` (required),
    `description?`, `size`, `className` and the Radix Content props.
  - 41 files import `DialogContent`. `components/ui/confirm-dialog.tsx` wraps it.
- **Connect journey stepper:** `components/apps/connect-journey/connect-journey.tsx:80` (`Steps`); a
  copy at `org-journey.tsx:200`. Both render the stepper as the first child of the dialog body. This
  brief only moves it into the header slot (see Scope 1); P5-POLISH-5 owns the journey otherwise.
- **`app/globals.css`** (559 lines):
  - `@import "tailwindcss"` :1, `@theme inline` :87, `@utility` :148–156.
  - Unlayered element rules: `html` :158, `body` :162, `h1,h2,h3` :171–176, `:focus-visible` :179–183
    (2 px accent outline plus a 5 px `--focus-halo` box-shadow), `::placeholder` :185,
    `a { color: inherit; }` :190–192, `[data-tour-active]` :195.
  - There is no `@layer base` or `@layer components`.
- **Button:** `components/ui/button.tsx`.
  - Variants `primary`, `outline`, `approve`, `ghost`, `danger`; sizes `sm`, `default`, `lg`.
  - `asChild` through Radix `Slot` (:31–35). `buttonVariants` is exported and applied straight to
    `<Link>` (the journey's "Open chat" does this).
- **Composer:** `components/chat/composer.tsx`.
  - Wrapper `focus-within:border-accent` :399.
  - The textarea removes the global ring at :440.
  - Other controls: `focus-visible:outline-2 outline-accent` :479, :550, :572.
  - The double ring the review saw is the global `:focus-visible` box-shadow halo plus the wrapper's
    accent border, together with the Write/Preview segmented control's ring.
- **Menus:** Radix dropdown-menu (`@radix-ui/react-dropdown-menu` ^2.2.1) and popover (1.3.1, pinned).
  Find the shared menu and popover wrappers in `components/ui/` (`dropdown-menu*`, `popover*`, `menu*`).
  The thick frame is the global `:focus-visible` rule applying to the focused menu content element.
- **CSP:**
  - The nonce is created in `src/proxy.ts:14` (CSP :21–27, `x-nonce` header :40), read in
    `app/layout.tsx:45` and applied to the theme script at :61.
  - The violation ("Applying inline style violates … style-src 'self' 'nonce-…'") fires when a menu
    or dialog opens. That is where `react-remove-scroll-bar` 2.3.8 (through Radix) injects a `<style>`
    element without a nonce; it reads the nonce from `get-nonce` (`setNonce`).
  - Confirm the source with the browser's console stack before fixing.
- **Design guards:** `web/tests/design-guards.test.ts` (tokens :70–83, no `style=` prop :105).

## Scope

1. **Dialog: three rows (A1, D-393).**
   - `DialogContent` becomes a flex column with `max-h-[calc(100dvh-2rem)]` and `overflow: hidden`.
     Its children:
     - a **header** row: title, optional description, close button, and a new optional `header` slot
       rendered under the title, used for steppers;
     - a **body**, the only element with `overflow-y-auto`;
     - an optional **footer** row from a new `footer` prop.
   - Padding moves onto the rows: header `px-6 pt-6 pb-3`, body `px-6 py-4`, footer `px-6 py-3` with a
     top border. Remove `sticky`, `-mx-6` and `-mt-6`.
   - Keep the public props working. Callers that render their own action row inside `children` keep
     working as before; they don't have to move it into `footer`.
   - Add `variant?: "centered" | "sheet"` (default `centered`):
     - `sheet` is a full-screen panel below 640 px (inset 0, no radius, header with a back slot and
       close, footer pinned to the bottom);
     - `sheet` is the same as `centered` above 640 px.
     - P5-POLISH-5 uses it for the journey.
   - Add `fixedHeight?: string` (for example `"600px"`). It gives the content that height on desktop,
     so multi-step dialogs don't re-centre between steps (B3); the body scrolls.
   - Move the journey's `<ol aria-label="Steps">` (both journeys) into the new `header` slot. Change
     nothing else in the journeys.
   - Check every one of the 41 callers at 1280 and 390 px in both themes. Fix any caller that relied on
     the old negative margins (for example custom `-mt-*` in children).
2. **CSS layers (A2, D-393).**
   - Wrap the element rules of `globals.css` in `@layer base { … }`: `html`, `body`, the headings,
     `::placeholder`, `a` and `[data-tour-active]`. Utilities then win over them. Keep their values.
   - Check that the token blocks (`:root`, `.dark`, `@theme inline`) still work. Tailwind v4 expects
     `@theme` at top level.
   - Sweep the rest of `web/src` for other unlayered global CSS (any `.css` file, and `<style>` in
     components) and layer it the same way.
3. **Focus (F3, D-393).**
   - Keep one focus ring: `:focus-visible` gets a 2 px `--accent` outline with a 2 px offset. Drop the
     5 px `--focus-halo` box-shadow from the global rule (the token stays; nothing else uses it as a
     ring).
   - No ring after a mouse click: nothing may style `:focus` without `-visible`. Grep for `focus:` and
     `:focus {` and change them.
   - **Composer:** only the outer container shows focus, by turning its border `--accent`
     (`focus-within`). The textarea, the Write/Preview segmented control and the toolbar buttons add
     no ring while the textarea has focus; the toolbar buttons keep a `:focus-visible` ring when they
     are themselves focused from the keyboard. Match `after/f3.png`.
   - **Menus and popovers:** the content element (`[role=menu]`, `[role=dialog]` of a popover,
     listbox) has `outline: none`. Items show keyboard focus with the `--subtle` background, as hover
     does. Dialogs keep the ring on their focused control, never on the dialog box.
4. **CSP nonce (F3).**
   - Make every style element a library injects carry the nonce. With `react-remove-scroll-bar`, call
     `setNonce(nonce)` from `get-nonce` once on the client, with the nonce the layout already has (pass
     it to a small client component in `app/layout.tsx`).
   - If the source turns out to be something else, fix that at its source the same way.
   - Do not add `'unsafe-inline'` to `style-src`.
5. **Guards.**
   - **Vitest:**
     - `design-guards.test.ts` fails on any unlayered element selector in `globals.css` (a selector
       outside `@layer`/`@theme`/`@utility` that is not a class, id, attribute or `:root`), and on
       `:focus` without `-visible` in component class strings and CSS;
     - a test renders each `Button` variant as `<a>` (through `asChild` and through `buttonVariants`
       on a `Link`) in both themes and asserts the computed text colour differs from the background,
       with contrast ≥ 4.5:1 computed from the token values.
   - **Playwright** (mocked API), new spec `e2e/dialogs.spec.ts`:
     - open a representative set of dialogs: the connect journey on each step, the account access
       dialog, the AI-key dialog, a confirm dialog, the reconnect dialog, the agent delete dialog;
     - at 1280 and 390 px, in both themes, assert that the bounding box of the body's first visible
       element starts at or below the header's bottom;
     - assert the close button is visible and inside the viewport after scrolling the body to its end;
     - assert no console error or CSP violation is logged while dialogs and menus open and close (fail
       on `securitypolicyviolation` events).
   - Add "no console errors" (CSP violations included) to the existing axe/narrow-route suites' common
     fixture, so every later spec inherits it.

## Out of scope

- The journey's content and steps (P5-POLISH-5), the account menu's content (P5-POLISH-8) and the
  account rows' menu (P5-POLISH-6). This brief fixes how dialogs and menus look and behave, not what
  they hold.
- Wording changes. A separate wording pass follows the P5-POLISH briefs.

## Constraints / file ownership

- **Owns:** `components/ui/dialog.tsx`, `components/ui/confirm-dialog.tsx`, the menu and popover
  wrappers in `components/ui/`, `components/ui/button.tsx` (only if needed), `app/globals.css`,
  `app/layout.tsx` (nonce client component), `components/chat/composer.tsx` (focus styles only),
  `tests/design-guards.test.ts`, `e2e/dialogs.spec.ts`.
- Touches the 41 dialog callers only to remove workarounds for the old header, and the two journeys
  only to move the stepper into the header slot.
- Tokens and shared primitives only (`14 §Visual design`). No new colours.

## Acceptance criteria

1. In the connect journey, every step shows its full step labels, and the Done step shows its
   confirmation sentence above its buttons (1280 and 390 px, both themes). Match `after/a1.png`.
2. "Open chat" (an `<a>` with the primary style) has readable text in both themes. Every Button
   variant rendered as a link passes the contrast test.
3. The composer, when focused, shows one accent border and no ring. An open menu has no outline. A
   mouse click leaves no ring. Keyboard focus is always visible.
4. Opening and closing menus and dialogs logs no CSP violation or console error, and the e2e suites now
   fail on one.
5. The new guards fail on the old code: an unlayered `a {}` rule, a sticky dialog header that overlaps
   the body, a `focus:` class.
6. Web gates: `pnpm typecheck`, lint (0 errors), vitest, `pnpm build`, the Playwright mocked suites
   (axe and narrow-route, both themes). Report the counts.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Shared primitives with 41 callers and a CSP fix that needs care; the
changes are small but must be checked everywhere they show.

## Review fixes (P5-POLISH-11 pass 1, lane `p5-polish-c`, 2026-11-19)

- **Close's focus ring (#1): done.** `DialogContent` handles `onOpenAutoFocus` itself: it focuses the first enabled
  input, textarea or select of the body, else the dialog; Close is no longer the autofocus target. A caller's own
  `onOpenAutoFocus` still runs first and wins by `preventDefault()`. Test: `tests/polish-11-review-fixes.test.tsx`
  (field focused, Close not; no field → the dialog).
