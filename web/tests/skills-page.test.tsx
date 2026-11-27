import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { json } from "./support/api";
import { wrap } from "./support/render";
import { AgentSkills, applied, effective, librarySource, me, menuItems, notApplied, openMenu, routes, skill } from "./support/skills";

// P5-SKILLS-3 (the Skills & Memory review, skill-1..3, "Also" 3; D-498, D-499, D-505, D-506): one list of everything an
// agent can do, the panel that reads first, and read-only skills that say so.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(),
}));

beforeEach(() => window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=skills"));

const style = skill({ name: "acme-style-guide", tier: "org", title: "Acme style guide", description: "write anything that leaves the team.", used_by_agents: 14 });
const draft = skill({ name: "draft-email", enabled: false });
const release = skill({ id: "own-release", name: "release-notes", description: "Write release notes the way Bob likes them.", body: "Group by theme.\nNo ticket numbers." });
const wsRelease = skill({ id: "ws-release", name: "release-notes", tier: "workspace", description: "Write release notes.", body: "Group by theme.\nKeep ticket numbers." });
const pdf = skill({ name: "pdf", description: "Read, merge, split and fill in PDF files.", source: librarySource({ update_pending: true }) });
const triage = skill({
  name: "triage-alerts",
  description: "Look at firing alerts and suggest who should look.",
  body: '# Triage alerts\n\nRun `scripts/firing.sh`.\n\n<img src=x onerror="window.__pwned=1">\n\n**Never silence an alert yourself.**',
  files: [{ path: "scripts/firing.sh", size: 130, executable: true }],
});
const minutes = skill({ name: "meeting-minutes", tier: "workspace", enabled: false, used_by_agents: 0 });
const appSkill = { ...skill({ name: "git-api", tier: "app", description: "REST conventions.", files: [{ path: "scripts/run.sh", size: 12, executable: true }] }), id: null, org_id: null, used_by_agents: null };
const app = { app_id: "git", name: "Git", version: "1.2.0" };

const review = () =>
  effective(
    [
      applied(style, { usage: { week: 0, last: null } }),
      applied(release, { shadows: [{ id: "ws-release", tier: "workspace" }] }),
      applied(pdf),
      applied(triage, { usage: { week: 3, last: { at: "2026-11-26T10:00:00Z", conversation: { id: "c1", title: "Alert storm 26 Nov" } } } }),
      applied(appSkill as never, { app, usage: null }),
    ],
    [
      notApplied(wsRelease, { shadowed_by: { id: "own-release", tier: "workspace" } }),
      notApplied(draft, { reason: "disabled" }),
      notApplied(minutes, { reason: "disabled" }),
    ],
  );

const base = (extra: Parameters<typeof routes>[0] = {}) => ({
  "GET /v1/me": () => json(200, me()),
  "GET /v1/agents/a1/skills/effective": review(),
  "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [] }),
  "GET /v1/agents/a1/skill-library-updates": () => json(200, { items: [] }),
  ...extra,
});

// the shadowed entry names its winner by id
const REPLACED = () => ({
  ...base(),
  "GET /v1/agents/a1/skills/effective": effective(
    [applied(release, { shadows: [{ id: "ws-release", tier: "workspace" }] })],
    [notApplied(wsRelease, { shadowed_by: { id: "own-release", tier: "agent" } })],
  ),
});

const row = (name: string) => screen.getByTestId(`skill-row-${name}`);
const list = () => screen.findByRole("list", { name: "Skills" });
const open = async (name: string) => {
  await list();
  fireEvent.click(within(row(name)).getAllByRole("button")[0]!);
};
const panel = (name: string) => screen.findByRole("article", { name });

describe("one list (skill-1)", () => {
  it("shows every skill once, sorted by name, by display name with the description as a sentence", async () => {
    routes(base());
    wrap(<AgentSkills />);
    const items = within(await list()).getAllByRole("listitem");
    expect(items.map((i) => within(i).getAllByRole("button")[0]!.textContent)).toEqual([
      "Acme style guide",
      "Draft email",
      "Git api",
      "Meeting minutes",
      "pdf",
      "Release notes",
      "Triage alerts",
    ]);
    // a sentence with its first letter capitalized, never glued to a label
    expect(within(row("acme-style-guide")).getByText("Write anything that leaves the team.")).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/Use it when/);
    expect(screen.queryByText("Skills this agent receives")).toBeNull();
    expect(screen.queryByText("Not applied")).toBeNull();
    for (const column of ["Skill", "Source", "For Daneel"]) expect(screen.getByText(column)).toBeTruthy();
  });

  it("gives each row one source chip and a switch, On or Off; an app skill reads On with nothing to switch", async () => {
    routes(base());
    wrap(<AgentSkills />);
    await list();
    await waitFor(() => expect(within(row("acme-style-guide")).getByTestId("source-chip").textContent).toBe("Acme"));
    expect(within(row("draft-email")).getByTestId("source-chip").textContent).toBe("Daneel");
    expect(within(row("meeting-minutes")).getByTestId("source-chip").textContent).toBe("Workspace");
    expect(within(row("pdf")).getByTestId("source-chip").textContent).toBe("Library · Anthropic skills");
    expect(within(row("git-api")).getByTestId("source-chip").textContent).toBe("App");
    expect(within(row("acme-style-guide")).getByRole("switch", { name: "Turn off for Daneel: Acme style guide" })).toBeChecked();
    expect(within(row("draft-email")).getByRole("switch")).not.toBeChecked();
    expect(within(row("draft-email")).getByText("Off")).toBeTruthy();
    expect(within(row("git-api")).queryByRole("switch")).toBeNull();
    expect(within(row("git-api")).getByText("On")).toBeTruthy();
  });

  it("what its owners switched off cannot be switched on for one agent, and says why", async () => {
    routes(base());
    wrap(<AgentSkills />);
    await list();
    const sw = within(row("meeting-minutes")).getByRole("switch");
    expect(sw).toBeDisabled();
    expect(sw).not.toBeChecked();
    expect(within(row("meeting-minutes")).getAllByText("Turned off by the workspace’s owners").length).toBeGreaterThan(0);
  });

  it("counts the chips and filters by them; Off holds what is off", async () => {
    routes(base());
    wrap(<AgentSkills />);
    await list();
    await waitFor(() => expect(screen.getByRole("radio", { name: "From Acme 1" })).toBeTruthy());
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual(["All 7", "Daneel’s own 4", "From the workspace 1", "From Acme 1", "Off 2"]);
    fireEvent.click(screen.getByRole("radio", { name: "Off 2" }));
    expect(within(await list()).getAllByRole("listitem")).toHaveLength(2);
    expect(screen.getByTestId("skill-row-draft-email")).toBeTruthy();
    expect(screen.getByTestId("skill-row-meeting-minutes")).toBeTruthy();
  });

  it("searches the name and the description", async () => {
    routes(base());
    wrap(<AgentSkills />);
    await list();
    fireEvent.change(screen.getByRole("searchbox", { name: "Search skills" }), { target: { value: "firing" } });
    expect(within(await list()).getAllByRole("listitem")).toHaveLength(1);
    fireEvent.change(screen.getByRole("searchbox", { name: "Search skills" }), { target: { value: "zzz" } });
    expect(screen.getByText("No skills match.")).toBeTruthy();
  });

  it("turns an inherited skill off for this agent only, and back on (C-1174), and an own skill by its own switch", async () => {
    let off = false;
    const calls = routes(
      base({
        "GET /v1/agents/a1/skills/effective": () =>
          off ? json(200, { items: [applied(draft)], shadowed: [notApplied(style, { reason: "off_for_agent", off_for_agent: true })] }) : json(200, { items: [applied(style)], shadowed: [notApplied(draft, { reason: "disabled" })] }),
        "PUT /v1/agents/a1/skills/acme-style-guide/off": () => {
          off = true;
          return json(204, null);
        },
        "DELETE /v1/agents/a1/skills/acme-style-guide/off": () => {
          off = false;
          return json(204, null);
        },
        "PATCH /v1/skills/draft-email": () => json(200, { ...draft, enabled: true }),
      }),
    );
    wrap(<AgentSkills />);
    await list();
    fireEvent.click(within(row("acme-style-guide")).getByRole("switch"));
    await waitFor(() => expect(within(row("acme-style-guide")).getByRole("switch")).not.toBeChecked());
    expect(screen.getByRole("radio", { name: "Off 1" })).toBeTruthy();
    expect(calls.filter((c) => c.method !== "GET").map((c) => `${c.method} ${c.path}`)).toEqual(["PUT /v1/agents/a1/skills/acme-style-guide/off"]);
    fireEvent.click(within(row("acme-style-guide")).getByRole("switch"));
    await waitFor(() => expect(within(row("acme-style-guide")).getByRole("switch")).toBeChecked());
    fireEvent.click(within(row("draft-email")).getByRole("switch"));
    await waitFor(() => expect(calls.some((c) => c.method === "PATCH" && c.path === "/v1/skills/draft-email" && c.body === '{"enabled":true}')).toBe(true));
  });

  it("says which version replaces which, in the row and under the list, and Compare shows the difference", async () => {
    routes(REPLACED());
    wrap(<AgentSkills />);
    await list();
    expect(within(row("release-notes")).getByText(/Daneel’s version replaces the workspace’s/)).toBeTruthy();
    expect(screen.getByText(/Release notes, Daneel’s own version, wins over the workspace’s\./)).toBeTruthy();
    const how = screen.getByRole("button", { name: "How skills combine" });
    expect(how).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(how);
    expect(screen.getByText(/One skill per name\. Daneel’s own skill replaces a workspace skill/)).toBeVisible();
    fireEvent.click(within(row("release-notes")).getByRole("button", { name: "Compare Release notes" }));
    const dlg = await screen.findByRole("dialog", { name: "Compare Release notes" });
    const diff = within(dlg).getByRole("region", { name: "Diff of the instructions" }).textContent;
    expect(diff).toContain("- Keep ticket numbers.");
    expect(diff).toContain("+ No ticket numbers.");
  });

  it("an emergency reader sees the agent's own skills only, with nothing to change", async () => {
    const calls = routes(base({ "GET /v1/agents/a1/skills": () => json(200, { items: [triage], next_cursor: null }) }));
    wrap(<AgentSkills readOnly />);
    await list();
    expect(within(row("triage-alerts")).getByRole("switch")).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Add skill" })).toBeNull();
    await open("triage-alerts");
    const p = await panel("Triage alerts");
    expect(within(p).queryByRole("button", { name: "Edit" })).toBeNull();
    expect(within(p).queryByRole("button", { name: /More actions/ })).toBeNull();
    expect(within(p).queryByRole("link", { name: "Try it in chat" })).toBeNull();
    expect(within(p).getByText("You can read this skill. You can’t change it.")).toBeTruthy();
    expect(calls.some((c) => c.path.endsWith("/effective"))).toBe(false);
  });

  it("with no skill at all it says so once, and Add skill is the way to start", async () => {
    routes(base({ "GET /v1/agents/a1/skills/effective": effective([]) }));
    wrap(<AgentSkills />);
    expect(await screen.findByRole("heading", { name: "No skills yet" })).toBeTruthy();
    openMenu("Add skill");
    // no library is connected: nothing to add from one
    expect(menuItems()).toEqual(["Write a new skill", "Upload a skill folder"]);
  });
});

describe("the panel (skill-2)", () => {
  it("opens beside the list, is held in the address, and reads first: rendered instructions and no fields", async () => {
    routes(base());
    const { container } = wrap(<AgentSkills />);
    await open("triage-alerts");
    const p = await panel("Triage alerts");
    expect(window.location.search).toContain("skill=triage-alerts");
    expect(within(p).getByText("Use it to look at firing alerts and suggest who should look.")).toBeTruthy();
    expect(within(p).getByRole("heading", { name: "Triage alerts", level: 1 })).toBeTruthy();
    expect(within(p).getByText("Never silence an alert yourself.").tagName).toBe("STRONG");
    // the shared renderer sanitizes: raw HTML keeps no handler
    expect(container.querySelector("[onerror]")).toBeNull();
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
    expect(within(p).queryByRole("textbox")).toBeNull();
    expect(within(p).getByRole("button", { name: "Edit" })).toBeTruthy();
    // the compact list keeps the switches
    expect(within(row("acme-style-guide")).getByRole("switch")).toBeTruthy();
    expect(screen.queryByText("For Daneel")).toBeNull();
  });

  it("Try it in chat opens the chat with the draft", async () => {
    routes(base());
    wrap(<AgentSkills />);
    await open("triage-alerts");
    const link = within(await panel("Triage alerts")).getByRole("link", { name: "Try it in chat" });
    const url = new URL(link.getAttribute("href")!, "http://localhost");
    expect(url.pathname).toBe("/o/acme/w/main/agents/daneel");
    expect(url.searchParams.get("tab")).toBe("chat");
    expect(url.searchParams.get("draft")).toBe("Use the “Triage alerts” skill to ");
  });

  it("lists the files as a tree and opens each in a code view through the file route (C-1099)", async () => {
    const calls = routes(base({ "GET /v1/skills/triage-alerts/files/scripts/firing.sh": () => new Response("#!/usr/bin/env bash\necho <b>x</b>\n") }));
    wrap(<AgentSkills />);
    await open("triage-alerts");
    const p = await panel("Triage alerts");
    const files = within(p).getByRole("region", { name: "Files · 2" });
    expect(within(files).getByText("scripts/")).toBeTruthy();
    const file = within(files).getByRole("button", { name: /^scripts\/firing\.sh/ });
    expect(within(file).getByText("Executable")).toBeTruthy();
    expect(within(file).getByText("130 chars")).toBeTruthy();
    expect(calls.some((c) => c.path.includes("/files/"))).toBe(false);
    fireEvent.click(file);
    const view = await within(files).findByRole("region", { name: "File scripts/firing.sh" });
    expect(view.textContent).toContain("echo <b>x</b>");
    expect(view.querySelector("b")).toBeNull();
    fireEvent.click(within(files).getByRole("button", { name: /^SKILL\.md/ }));
    expect(within(files).getByRole("region", { name: "File SKILL.md" }).textContent).toContain("# Triage alerts");
  });

  it("says how it was used, naming the conversation only when the server does", async () => {
    routes(base());
    wrap(<AgentSkills />);
    await open("triage-alerts");
    expect(within(await panel("Triage alerts")).getByTestId("skill-usage").textContent).toBe("Used 3 times this week · last in “Alert storm 26 Nov”");
  });

  it.each([
    [{ week: 1, last: { at: "2026-11-26T10:00:00Z", conversation: null } }, "Used 1 time this week · last in a conversation"],
    [{ week: 2, last: { at: "2026-11-26T10:00:00Z", conversation: { id: "c1", title: null } } }, "Used 2 times this week · last in a conversation"],
    [{ week: 0, last: null }, "Not used yet"],
  ])("usage %j reads %s", async (usage, words) => {
    routes(base({ "GET /v1/agents/a1/skills/effective": effective([applied(triage, { usage })]) }));
    wrap(<AgentSkills />);
    await open("triage-alerts");
    expect(within(await panel("Triage alerts")).getByTestId("skill-usage").textContent).toBe(words);
  });

  it("its … menu removes the agent's own skill after a confirm", async () => {
    const calls = routes(base({ "DELETE /v1/skills/triage-alerts": () => json(204, null) }));
    wrap(<AgentSkills />);
    await open("triage-alerts");
    await panel("Triage alerts");
    openMenu("More actions for Triage alerts");
    expect(menuItems()).toEqual(["Remove from Daneel…"]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove from Daneel…" }));
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    fireEvent.click(within(await screen.findByRole("dialog", { name: "Remove “Triage alerts” from Daneel?" })).getByRole("button", { name: "Remove skill" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/skills/triage-alerts")).toBe(true));
  });
});

describe("read-only skills say so (skill-3)", () => {
  const noFields = (p: HTMLElement) => {
    expect(within(p).queryAllByRole("textbox")).toEqual([]);
    expect(within(p).queryByRole("button", { name: "Save" })).toBeNull();
    expect(within(p).queryByRole("button", { name: "Edit" })).toBeNull();
    expect(within(p).queryAllByRole("switch").length).toBeLessThanOrEqual(1);
  };

  it("an organization's skill: the lock line with the organization's words, usage across agents, no fields", async () => {
    routes(base());
    wrap(<AgentSkills />);
    await open("acme-style-guide");
    const p = await panel("Acme style guide");
    await waitFor(() => expect(within(p).getByTestId("skill-lock").textContent).toBe("Managed by Acme’s admins. You can turn it off for Daneel."));
    noFields(p);
    expect(within(p).getByTestId("skill-usage").textContent).toBe("Used by 14 agents in Acme · Daneel used it 0 times this week");
    expect(within(p).getByRole("link", { name: "Try it in chat" })).toBeTruthy();
  });

  it("a workspace skill is never labelled with the organization's words (S-4)", async () => {
    const ws = skill({ name: "incident-postmortem", tier: "workspace", used_by_agents: 1 });
    routes(base({ "GET /v1/agents/a1/skills/effective": effective([applied(ws)]) }));
    wrap(<AgentSkills />);
    await open("incident-postmortem");
    const p = await panel("Incident postmortem");
    expect(within(p).getByTestId("skill-lock").textContent).toBe("Managed by the workspace’s owners. You can turn it off for Daneel.");
    expect(within(p).getByTestId("skill-usage").textContent).toBe("Used by 1 agent in this workspace · Daneel used it 0 times this week");
    expect(p.textContent).not.toMatch(/organization|Acme/);
  });

  it("an app's skill comes with the app: no switch, no Try-less menu for one with files", async () => {
    routes(base());
    wrap(<AgentSkills />);
    await open("git-api");
    const p = await panel("Git api");
    expect(within(p).getByTestId("skill-lock").textContent).toBe("Comes with the Git app.");
    noFields(p);
    expect(within(p).queryByRole("switch")).toBeNull();
    // its files have no route to read them by: listed, not opened
    expect(within(p).getByText("run.sh")).toBeTruthy();
    expect(within(p).queryByRole("button", { name: /run\.sh/ })).toBeNull();
    expect(within(p).queryByRole("button", { name: /More actions/ })).toBeNull();
    expect(within(p).queryByTestId("skill-usage")).toBeNull();
  });

  const waiting = {
    "GET /v1/skills/pdf/library": () =>
      json(200, {
        commit_sha: "a".repeat(40), pinned_at: "2026-11-01T00:00:00Z", detached: false,
        update: {
          id: "u1", kind: "changed", commit_sha: "b".repeat(40), created_at: "2026-11-20T00:00:00Z", content_sha256: "c".repeat(64), over_budget: false,
          diff: {
            description: { old: "a", new: "a", changed: false }, body: { old: "a", new: "b", changed: true },
            files: [{ path: "forms.md", change: "changed", old: "a", new: "b", binary: false }, { path: "merge.py", change: "added", new: "b", binary: false }, { path: "same.md", change: "unchanged", binary: false }],
          },
        },
      }),
  };

  it("a library skill says where it comes from and its pin; an update offers Review update to who may approve", async () => {
    routes(base(waiting));
    wrap(<AgentSkills />);
    await open("pdf");
    const p = await panel("pdf");
    expect(within(p).getByTestId("skill-lock").textContent).toBe("From Anthropic skills · pinned to e4bb5d9. Changes come from the library, not from here.");
    noFields(p);
    const update = within(p).getByTestId("update-available");
    await waitFor(() => expect(update.textContent).toContain("3 files changed upstream."));
    fireEvent.click(within(update).getByRole("button", { name: "Review update" }));
    expect(await screen.findByRole("dialog", { name: "Update to pdf" })).toBeTruthy();
  });

  it("…and nothing about the update to someone who cannot approve it", async () => {
    const orgPdf = skill({ name: "internal-comms", tier: "org", source: librarySource({ label: "Acme team recipes", update_pending: true }) });
    const calls = routes(base({ "GET /v1/agents/a1/skills/effective": effective([applied(orgPdf)]) }));
    wrap(<AgentSkills />);
    await open("internal-comms");
    const p = await panel("Internal comms");
    await waitFor(() => expect(within(p).getByTestId("skill-lock").textContent).toBe("Managed by Acme’s admins. You can turn it off for Daneel.From Acme team recipes · pinned to e4bb5d9."));
    expect(within(p).queryByTestId("update-available")).toBeNull();
    expect(within(p).queryByRole("button", { name: "Review update" })).toBeNull();
    expect(calls.some((c) => c.path.endsWith("/library"))).toBe(false);
    expect(within(p).getByTestId("source-chip").textContent).toBe("Acme · Library");
  });

  it("an organization admin may review the same update from the agent's page", async () => {
    const orgPdf = skill({ id: "pdf", name: "pdf", tier: "org", source: librarySource({ update_pending: true }) });
    routes(base({ ...waiting, "GET /v1/me": () => json(200, me("admin")), "GET /v1/agents/a1/skills/effective": effective([applied(orgPdf)]) }));
    wrap(<AgentSkills />);
    await open("pdf");
    expect(await within(await panel("pdf")).findByRole("button", { name: "Review update" })).toBeTruthy();
  });

  it("Stop updates and edit explains itself, then opens the editor; it is never a button beside Remove", async () => {
    let detached = false;
    const calls = routes(
      base({
        "GET /v1/agents/a1/skills/effective": () => json(200, { items: [applied(detached ? { ...pdf, source: librarySource({ detached: true, library_id: null }) } : pdf)], shadowed: [] }),
        "POST /v1/skills/pdf/library/detach": () => {
          detached = true;
          return json(200, pdf);
        },
      }),
    );
    wrap(<AgentSkills />);
    await open("pdf");
    await panel("pdf");
    expect(screen.queryByRole("button", { name: /Stop updates|Remove|Detach/ })).toBeNull();
    openMenu("More actions for pdf");
    expect(menuItems()).toEqual(["Stop updates and edit", "Remove from Daneel…"]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Stop updates and edit" }));
    const dlg = await screen.findByRole("dialog", { name: "Stop updates and edit “pdf”?" });
    expect(dlg.textContent).toContain("The library no longer proposes updates to it.");
    expect(calls.some((c) => c.method === "POST")).toBe(false);
    fireEvent.click(within(dlg).getByRole("button", { name: "Stop updates and edit" }));
    expect(await screen.findByRole("form", { name: "Edit pdf" })).toBeTruthy();
  });

  it("Duplicate as the agent's own copies the skill and its files, switched on only when whole", async () => {
    const withFile = skill({ name: "meeting-minutes", tier: "workspace", title: "Meeting minutes", files: [{ path: "template.md", size: 5, executable: false }] });
    const calls = routes(
      base({
        "GET /v1/agents/a1/skills/effective": effective([applied(withFile)]),
        "GET /v1/skills/meeting-minutes/files/template.md": () => new Response("# T\n"),
        "POST /v1/agents/a1/skills": () => json(201, skill({ id: "copy", name: "meeting-minutes", enabled: false })),
        "PUT /v1/skills/copy/files/template.md": () => json(200, skill({ id: "copy" })),
        "PATCH /v1/skills/copy": () => json(200, skill({ id: "copy" })),
      }),
    );
    wrap(<AgentSkills />);
    await open("meeting-minutes");
    await panel("Meeting minutes");
    openMenu("More actions for Meeting minutes");
    expect(menuItems()).toEqual(["Duplicate as Daneel’s own"]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate as Daneel’s own" }));
    await waitFor(() => expect(calls.filter((c) => c.method !== "GET").map((c) => `${c.method} ${c.path}`)).toEqual(["POST /v1/agents/a1/skills", "PUT /v1/skills/copy/files/template.md", "PATCH /v1/skills/copy"]));
    expect(JSON.parse(calls.find((c) => c.method === "POST")!.body)).toMatchObject({ name: "meeting-minutes", title: "Meeting minutes", enabled: false });
    expect(calls.find((c) => c.method === "PUT")!.body).toBe("# T\n");
    expect(calls.find((c) => c.method === "PATCH")!.body).toBe('{"enabled":true}');
  });
});

// A library's or an app's SKILL.md is text an outsider wrote: showing it must not make the reader's browser fetch
// anything, from this origin (with the reader's session) or from anywhere else.
describe("text from a library or an app fetches nothing", () => {
  const BODY = [
    "![relative pixel](/v1/orgs/o1/skills)",
    "![outside pixel](https://tracker.example/pixel)",
    '<img src="/v1/me" alt="raw tag">',
    '<picture><source srcset="/v1/from-source 1x"><img src="/v1/in-picture" alt="in a picture"></picture>',
    "[![linked pixel](/v1/linked)](https://example.org/page)",
    "![](/v1/no-alt)",
  ].join("\n\n");
  const fetchers = "img, source, picture, video, audio, iframe, object, embed, [srcset], input[type=image]";

  const check = (container: HTMLElement) => {
    expect([...container.querySelectorAll(fetchers)].map((e) => e.outerHTML)).toEqual([]);
    for (const alt of ["relative pixel", "outside pixel", "raw tag", "in a picture", "linked pixel"]) {
      expect(container.textContent).toContain(`image: ${alt}`);
    }
    // a link stays a link, opened only when pressed
    expect(within(container).getByRole("link", { name: /image: linked pixel/ })).toHaveAttribute("rel", "noopener noreferrer nofollow");
  };

  const app = { app_id: "git", name: "Git", version: "1.2.0" };
  it.each([
    ["a library skill", { source: librarySource() }, {}],
    ["a library skill whose updates were stopped", { source: librarySource({ detached: true, library_id: null }) }, {}],
    ["an app skill", { id: null, org_id: null, tier: "app", scope: "app" }, { app }],
  ])("%s renders no image, only its alt text", async (_what, over, extra) => {
    const foreign = skill({ name: "foreign", body: BODY, ...over });
    routes(base({ "GET /v1/agents/a1/skills/effective": effective([applied(foreign, extra)]) }));
    wrap(<AgentSkills />);
    await open("foreign");
    check(within(await panel("foreign")).getByRole("region", { name: "Instructions" }));
  });

  it("nor in the editor's preview of a library skill whose updates were stopped", async () => {
    const stopped = skill({ name: "foreign", body: BODY, source: librarySource({ detached: true, library_id: null }) });
    routes(base({ "GET /v1/agents/a1/skills/effective": effective([applied(stopped)]) }));
    wrap(<AgentSkills />);
    await open("foreign");
    fireEvent.click(within(await panel("foreign")).getByRole("button", { name: "Edit" }));
    const form = await screen.findByRole("form", { name: "Edit foreign" });
    fireEvent.click(within(form).getByRole("tab", { name: "Preview" }));
    check(within(form).getByRole("tabpanel", { name: "Preview" }));
  });

  it("a skill written in Aurora shows a same-origin picture, as chat and Memory do", async () => {
    routes(base({ "GET /v1/agents/a1/skills/effective": effective([applied(skill({ name: "mine", body: "![chart](/v1/files/chart)" }))]) }));
    wrap(<AgentSkills />);
    await open("mine");
    expect(within(await panel("mine")).getByRole("img", { name: "chart" })).toHaveAttribute("src", "/v1/files/chart");
  });
});

describe("app skills (D-455, C-1133)", () => {
  const body = (name: string) => ({ ...skill({ name, tier: "app", description: "d", body: '# x\n\n<img src=x onerror="window.__pwned=1">' }), id: null, org_id: null, used_by_agents: null });
  const jira = { app_id: "jira", name: "Jira", version: "1.0.0" };
  const jiraDc = { app_id: "jira-dc", name: "Jira Data Center", version: "2.0.0" };

  it("an app's skill that replaces another app's names both; its text never runs", async () => {
    routes(
      base({
        "GET /v1/agents/a1/skills/effective": effective(
          [applied(body("jira-x") as never, { app: jiraDc, usage: null, shadows: [{ tier: "app", id: null, app_id: "jira" }] })],
          [notApplied(body("jira-x") as never, { app: jira, usage: null, shadowed_by: { tier: "app", id: null, app_id: "jira-dc" } })],
        ),
      }),
    );
    const { container } = wrap(<AgentSkills />);
    expect(within(await list()).getAllByRole("listitem")).toHaveLength(1);
    expect(within(row("jira-x")).getByText(/The Jira Data Center app’s version replaces the Jira app’s/)).toBeTruthy();
    await open("jira-x");
    await panel("Jira x");
    expect(container.querySelector("[onerror]")).toBeNull();
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
  });

  it("an app skill without files can be copied as the agent's own", async () => {
    const calls = routes(
      base({
        "GET /v1/agents/a1/skills/effective": effective([applied(body("git-notes") as never, { app: jira, usage: null })]),
        "POST /v1/agents/a1/skills": () => json(201, skill({ id: "copy", name: "git-notes" })),
      }),
    );
    wrap(<AgentSkills />);
    await open("git-notes");
    await panel("Git notes");
    openMenu("More actions for Git notes");
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate as Daneel’s own" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    expect(JSON.parse(calls.find((c) => c.method === "POST")!.body)).toMatchObject({ name: "git-notes", enabled: true });
  });
});
