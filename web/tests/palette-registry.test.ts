import { describe, expect, it } from "vitest";

import { matchItems, matchScore, paletteItems, paletteVerbs, type PaletteContext } from "@/lib/palette-registry";

const base: PaletteContext = {
  verbs: true,
  orgSlug: "acme",
  wsSlug: "ops",
  agentHandle: "daneel",
  memberships: [
    { slug: "acme", name: "Acme", role: "owner", workspaces: [{ slug: "ops", name: "Operations" }, { slug: "labs", name: "Labs" }] },
    { slug: "globex", name: "Globex", role: "member", workspaces: [{ slug: "main", name: "Main" }] },
  ],
  topics: ["overview", "agents", "skills", "apps", "approvals", "activity", "people", "settings"],
  agents: [
    { id: "a1", name: "Daneel", handle: "daneel", paused: false, byo: false },
    { id: "a2", name: "Giskard", handle: "giskard", paused: true, byo: false },
  ],
  agentPages: [{ id: "chat", label: "Chat" }, { id: "instructions", label: "Instructions" }],
  role: "owner",
  instanceAdmin: false,
  ssh: true,
};

const labels = (items: { label: string }[]) => items.map((i) => i.label);

describe("palette items", () => {
  it("lists the nouns for everyone and no verbs at Use", () => {
    const items = paletteItems({ ...base, verbs: false });
    expect(items.some((i) => i.group === "Actions")).toBe(false);
    const names = labels(items);
    for (const n of ["Operations", "Labs", "Main", "Manage Acme", "Daneel","Giskard", "Overview", "Approvals", "Chat", "Instructions", "Your account", "API keys", "SSH keys"]) {
      expect(names).toContain(n);
    }
  });

  it("(D-508(e), (f)) offers Manage only where the viewer may open it, and Administration to instance admins only", () => {
    expect(labels(paletteItems(base))).not.toContain("Manage Globex");
    expect(labels(paletteItems(base))).not.toContain("Administration");
    const admin = paletteItems({ ...base, instanceAdmin: true });
    expect(labels(admin)).toContain("Manage Globex");
    expect(admin.find((i) => i.label === "Administration")).toMatchObject({ group: "Places", href: "/admin" });
    // A role that is not known yet offers nothing: only an owner or admin has Manage.
    const unknown = { ...base, memberships: [{ slug: "acme", name: "Acme", workspaces: [] }] };
    expect(labels(paletteItems(unknown))).not.toContain("Manage Acme");
  });

  it("leaves SSH keys out when the instance has no SSH", () => {
    expect(labels(paletteItems({ ...base, ssh: false }))).not.toContain("SSH keys");
  });

  it("lists the pages the viewer's topics allow, and the open agent's pages only on an agent page", () => {
    const items = paletteItems({ ...base, verbs: false, topics: ["overview", "agents"], agentHandle: undefined });
    expect(labels(items)).not.toContain("Approvals");
    expect(items.some((i) => i.group === "Agent pages")).toBe(false);
  });

  it("lists verbs for an owner with the switch on, with the same destinations as the buttons", () => {
    const verbs = paletteVerbs(base);
    const l = labels(verbs);
    expect(l).toEqual(
      expect.arrayContaining([
        "New chat with Daneel",
        "Give Daneel an app",
        "New schedule for Daneel",
        "New webhook for Daneel",
        "Pause Daneel",
        "Export Daneel",
        "Import into Daneel",
        "Resume Giskard",
        "Open Approvals",
        "Add people",
        "New agent",
        "Keyboard shortcuts",
      ]),
    );
    const pause = verbs.find((v) => v.label === "Pause Daneel")!;
    expect(pause.verb).toBe("pause-agent");
    expect(pause.href).toBe("/o/acme/w/ops/agents/daneel?tab=manage");
    // (D-447) Export, Import and Resume open Manage, where their buttons are: Resume does not resume by itself.
    expect(verbs.find((v) => v.label === "Export Daneel")!.href).toBe("/o/acme/w/ops/agents/daneel?tab=manage");
    expect(verbs.find((v) => v.label === "Import into Daneel")!.href).toBe("/o/acme/w/ops/agents/daneel?tab=manage");
    const resume = verbs.find((v) => v.label === "Resume Giskard")!;
    expect(resume.verb).toBeUndefined();
    expect(resume.href).toBe("/o/acme/w/ops/agents/giskard?tab=manage");
  });

  it("leaves out the verbs a role may not use", () => {
    const member = labels(paletteVerbs({ ...base, role: "member" }));
    expect(member).toContain("New chat with Daneel");
    for (const gone of ["Pause Daneel", "Give Daneel an app", "New schedule for Daneel", "Open Approvals", "Add people", "New agent", "Export Daneel", "Import into Daneel"]) {
      expect(member).not.toContain(gone);
    }
    const approver = labels(paletteVerbs({ ...base, role: "approver" }));
    expect(approver).toContain("Open Approvals");
    expect(approver).not.toContain("Add people");
  });

  it("gives a BYO agent no chat, schedule, webhook or export verb", () => {
    const l = labels(paletteVerbs({ ...base, agents: [{ id: "b", name: "Hal", handle: "hal", paused: false, byo: true }] }));
    expect(l).toContain("Give Hal an app");
    expect(l).not.toContain("New chat with Hal");
    expect(l).not.toContain("New schedule for Hal");
    expect(l).not.toContain("Export Hal");
    expect(l).not.toContain("Import into Hal");
  });
});

describe("matching", () => {
  const items = paletteItems(base);

  it("is case-insensitive, by substring and by word start", () => {
    expect(matchScore({ label: "Pause Daneel" }, "dan")).toBeGreaterThan(0);
    expect(matchScore({ label: "Pause Daneel" }, "PAUSE")).toBeGreaterThan(0);
    expect(matchScore({ label: "Pause Daneel" }, "aneel")).toBeGreaterThan(0);
    expect(matchScore({ label: "Pause Daneel" }, "zzz")).toBe(0);
    expect(matchScore({ label: "New schedule for Daneel" }, "sched dan")).toBeGreaterThan(0);
  });

  it("puts a label that starts with the text above a word start above a substring", () => {
    const a = matchScore({ label: "Approvals" }, "app");
    const b = matchScore({ label: "Open Approvals" }, "app");
    const c = matchScore({ label: "Wrapper" }, "app");
    expect(a).toBeGreaterThan(b);
    expect(b).toBeGreaterThan(c);
  });

  it("finds a workspace by its organization's name through the hint", () => {
    expect(labels(matchItems(items, "globex", []).filter((i) => i.group === "Workspaces"))).toEqual(["Main"]);
  });

  it("returns nothing for text that matches nothing", () => {
    expect(matchItems(items, "qqqqq", [])).toEqual([]);
  });

  it("lists the most recently used workspace first when nothing is typed", () => {
    const ws = matchItems(items, "", ["acme/labs"]).filter((i) => i.group === "Workspaces");
    expect(labels(ws)[0]).toBe("Labs");
  });

  it("keeps one heading per group (a group is never split)", () => {
    const groups = matchItems(items, "a", []).map((i) => i.group);
    const seen: string[] = [];
    for (const g of groups) if (seen[seen.length - 1] !== g) seen.push(g);
    expect(new Set(seen).size).toBe(seen.length);
  });
});
