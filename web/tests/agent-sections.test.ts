import { describe, expect, it } from "vitest";

import {
  AGENT_PAGE_IDS,
  availableSections,
  defaultPage,
  hiddenPageCount,
  isPageAvailable,
  LEGACY_AUTOMATION_TAB,
  resolvePage,
  sectionLanding,
  SECTION_OF,
  type PageAvailability,
} from "@/components/agents/sections";

// D-200: five sections, the page ids of `?tab=` unchanged.

const hosted: PageAvailability = { byo: false, previews: false, terminal: false };
const byo: PageAvailability = { byo: true, previews: false, terminal: false };

describe("agent sections", () => {
  it("maps every legacy ?tab= id to its section", () => {
    expect(SECTION_OF).toEqual({
      chat: "chat",
      instructions: "settings",
      memory: "settings",
      skills: "settings",
      evals: "settings",
      automations: "triggers",
      schedules: "triggers",
      webhooks: "triggers",
      channels: "triggers",
      peers: "settings",
      computer: "computer",
      terminal: "computer",
      files: "computer",
      previews: "computer",
      ssh: "computer",
      connect: "computer",
      access: "settings",
    });
    expect(Object.keys(SECTION_OF).sort()).toEqual([...AGENT_PAGE_IDS].sort());
  });

  it("lists a hosted agent's sections in order (Chat · Automations · Computer, Settings behind its button), with Terminal and Previews only where enabled", () => {
    const s = availableSections(hosted);
    expect(s.map((x) => x.id)).toEqual(["chat", "triggers", "computer", "settings"]);
    expect(s.filter((x) => x.id !== "settings").map((x) => x.label)).toEqual(["Chat", "Automations", "Computer"]);
    expect(s.find((x) => x.id === "settings")!.pages.map((p) => p.label)).toEqual(["Instructions", "Apps & access", "Memory", "Skills", "Tests", "Other agents"]);
    expect(s.find((x) => x.id === "triggers")!.pages.map((p) => p.label)).toEqual(["Automations"]);
    // (P5-REFINE-15) No Files: the one page is the state card, named Overview.
    expect(s.find((x) => x.id === "computer")!.pages.map((p) => p.label)).toEqual(["Overview"]);
    const all = availableSections({ byo: false, previews: true, terminal: true });
    expect(all.find((x) => x.id === "computer")!.pages.map((p) => p.label)).toEqual(["Files", "Terminal", "Previews", "Details"]);
    // (P5-FILES-2, D-316(b)) Files follows the Terminal's switch exactly and has no gate of its own.
    const noTerminal = availableSections({ byo: false, previews: true, terminal: false });
    expect(noTerminal.find((x) => x.id === "computer")!.pages.map((p) => p.label)).toEqual(["Previews", "Overview"]);
    expect(isPageAvailable("files", { byo: true, previews: true, terminal: true })).toBe(false);
    expect(resolvePage("files", { byo: false, previews: false, terminal: false })).toBe("chat");
  });

  it("gives a BYO agent Computer (Computer: Connect) and Settings (Instructions, Apps & access) only", () => {
    const s = availableSections(byo);
    expect(s.map((x) => x.id)).toEqual(["computer", "settings"]);
    expect(s[0]!.pages.map((p) => [p.id, p.label])).toEqual([["connect", "Computer: Connect"]]);
    expect(s[1]!.pages.map((p) => p.id)).toEqual(["instructions", "access"]);
    expect(isPageAvailable("chat", byo)).toBe(false);
    expect(isPageAvailable("memory", byo)).toBe(false);
    expect(isPageAvailable("connect", hosted)).toBe(false);
  });

  it("hides a section whose pages are all unavailable", () => {
    expect(availableSections({ byo: true, previews: true, terminal: true }).some((x) => x.id === "triggers")).toBe(false);
  });

  it("opens a hosted agent on Chat and a BYO agent on Instructions; deep links keep their page", () => {
    expect(defaultPage(hosted)).toBe("chat");
    expect(defaultPage(byo)).toBe("instructions");
    expect(resolvePage(null, hosted)).toBe("chat");
    expect(resolvePage("memory", hosted)).toBe("memory");
    expect(resolvePage("access", hosted)).toBe("access");
    expect(resolvePage("instructions", hosted)).toBe("instructions");
    expect(resolvePage("chat", byo)).toBe("instructions");
    expect(resolvePage("memory", byo)).toBe("instructions");
    expect(resolvePage("terminal", hosted)).toBe("chat");
    expect(resolvePage("terminal", { ...hosted, terminal: true })).toBe("terminal");
    expect(resolvePage("bogus", hosted)).toBe("chat");
  });

  it("the old Schedules, Webhooks and Channels ids open the one Automations page (P5-REFINE-14)", () => {
    for (const id of ["automations", "schedules", "webhooks", "channels"]) expect(resolvePage(id, hosted), id).toBe("automations");
    for (const id of ["automations", "schedules", "webhooks", "channels"]) expect(resolvePage(id, byo), id).toBe("instructions");
    expect(LEGACY_AUTOMATION_TAB).toEqual({ schedules: "schedules", webhooks: "webhooks", channels: "chats" });
  });
});

describe("agent sections by altitude (D-406)", () => {
  const all: PageAvailability = { byo: false, previews: true, terminal: true, ssh: true };
  const at = (floor: "use" | "setup" | "expert") => (level: "setup" | "expert") => level === "setup" ? floor !== "use" : floor === "expert";
  const labels = (s: ReturnType<typeof availableSections>, id: string) => s.find((x) => x.id === id)!.pages.map((p) => p.label);

  it("at Use lists Instructions, Apps & access, Automations, and the Computer's Files and Details only", () => {
    const s = availableSections(all, at("use"));
    expect(labels(s, "settings")).toEqual(["Instructions", "Apps & access", "Memory", "Skills"]);
    expect(labels(s, "triggers")).toEqual(["Automations"]);
    expect(labels(s, "computer")).toEqual(["Files", "Details"]);
  });
  it("opens the Computer on Files when there are Files, else on Overview (D-408)", () => {
    const pages = (av: PageAvailability) => availableSections(av).find((x) => x.id === "computer")!.pages;
    expect(sectionLanding("computer", pages(all))).toBe("files");
    expect(sectionLanding("computer", pages({ ...all, terminal: false }))).toBe("computer");
    expect(sectionLanding("chat", [{ id: "chat" }])).toBe("chat");
  });
  it("at Set up lists the same settings pages as Use; at Expert everything", () => {
    expect(labels(availableSections(all, at("setup")), "settings")).toEqual(["Instructions", "Apps & access", "Memory", "Skills"]);
    const s = availableSections(all, at("expert"));
    expect(labels(s, "settings")).toEqual(["Instructions", "Apps & access", "Memory", "Skills", "Tests", "Other agents"]);
    expect(labels(s, "computer")).toEqual(["Files", "Terminal", "Connect", "Previews", "Details"]);
  });
  it("keeps the open page listed, and a direct ?tab= still resolves to it", () => {
    expect(resolvePage("evals", hosted)).toBe("evals");
    expect(labels(availableSections(hosted, at("use"), "evals"), "settings")).toEqual(["Instructions", "Apps & access", "Memory", "Skills", "Tests"]);
  });
  it("counts the pages the altitude leaves out", () => {
    expect(hiddenPageCount(all, at("use"))).toBe(5);
    expect(hiddenPageCount(all, at("expert"))).toBe(0);
  });
  it("lists a BYO agent's Computer: Connect only at Expert", () => {
    expect(availableSections(byo, at("use")).map((x) => x.id)).toEqual(["settings"]);
    expect(availableSections(byo, at("expert")).map((x) => x.id)).toEqual(["computer", "settings"]);
  });
});
