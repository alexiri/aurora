# Dashboard visual design — reference mock-ups

Static HTML mock-ups for the dashboard visual design (`14 §Visual design`,
decision D-68, task `P2-UI`). They are exported from the owner's Claude Design
canvas. They are **reference material, not code**: open them in a browser to
see the intended look, and read the inline styles for exact values. The
dashboard never serves them and nothing imports them.

The logo files (master logos and the variants generated from them) are in
[`brand/`](brand/README.md). The mock-ups load them from there.

Source canvas (owner's Claude Design project, private):
https://claude.ai/artifact/P7qkWpLXqMnfAQLHzM98U8

| Light | Dark | Screen | What it restyles |
|---|---|---|---|
| `Foundations.html` | `FoundationsDark.html` | Tokens, type, buttons, badges, fields, tabs, theme switch | `web/src/app/globals.css`, `web/src/components/ui/*` |
| `Brand.html` | — | Lockups, app icons, usage rules | `brand/`, `AuroraMark`, `AuroraLogo`, favicon and app icons |
| `Overview.html` | `OverviewDark.html` | Workspace overview inside the sidebar shell | `/o/<org>/w/<ws>` + `app-shell.tsx` |
| `Approvals.html` | `ApprovalsDark.html` | Pending approvals (batch group, cards, conflict state) | `/o/<org>/w/<ws>/approvals` |
| `Agent.html` | `AgentDark.html` | Agent page, Chat tab, access side panel | `/o/<org>/w/<ws>/agents/<handle>` |
| `SignIn.html` | — | Auth pages (split layout with brand panel) | `auth-card.tsx` and `/auth/*` |

The theme switch in each sidebar links the light and dark version of the same
screen.

Where a mock-up and the specification disagree, **the specification wins**.
Known differences:

- `SignIn.html` shows an email/password form. From phase 1 (RFE-004) sign-in
  is OIDC provider buttons only; adopt the layout, not the form.
- Names, requests, counts and the "Running / Idle" status on agent cards are
  sample data. The `Agent` schema has no runtime state, so agent cards show
  status only once the API provides it (see `14 §Visual design`).
- The "Coming soon" nav group and the "Search or jump to… ⌘K" button in
  `Overview.html` are not part of the design to implement.
- In the mock-ups the theme switch is made of links. In the dashboard it is a
  real radio group that changes the theme in place (I-41).
- The mock-ups show the logo with `<img>`. The dashboard renders it inline
  (`AuroraLogo`) so it follows the theme.
- The mock-ups use Google Fonts and inline `style` attributes. The dashboard
  self-hosts fonts (I-40) and uses Tailwind classes. The production CSP blocks
  inline style attributes.
