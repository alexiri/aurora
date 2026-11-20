import { describe, expect, it } from "vitest";

import {
  AGENT_PAGE_IDS,
  availableSections,
  defaultPage,
  isPageAvailable,
  resolvePage,
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
      instructions: "behaviour",
      memory: "behaviour",
      skills: "behaviour",
      evals: "behaviour",
      schedules: "triggers",
      webhooks: "triggers",
      channels: "triggers",
      peers: "triggers",
      computer: "computer",
      terminal: "computer",
      files: "computer",
      previews: "computer",
      ssh: "computer",
      connect: "computer",
      access: "access",
    });
    expect(Object.keys(SECTION_OF).sort()).toEqual([...AGENT_PAGE_IDS].sort());
  });

  it("lists a hosted agent's five sections in order, with Terminal and Previews only where enabled", () => {
    const s = availableSections(hosted);
    expect(s.map((x) => x.id)).toEqual(["chat", "behaviour", "triggers", "computer", "access"]);
    expect(s.find((x) => x.id === "behaviour")!.pages.map((p) => p.label)).toEqual(["Instructions", "Memory", "Skills", "Tests"]);
    expect(s.find((x) => x.id === "triggers")!.pages.map((p) => p.label)).toEqual(["Schedules", "Webhooks", "Channels", "Other agents"]);
    expect(s.find((x) => x.id === "computer")!.pages.map((p) => p.label)).toEqual(["Computer"]);
    const all = availableSections({ byo: false, previews: true, terminal: true });
    expect(all.find((x) => x.id === "computer")!.pages.map((p) => p.label)).toEqual(["Computer", "Terminal", "Files", "Previews"]);
    // (P5-FILES-2, D-316(b)) Files follows the Terminal's switch exactly and has no gate of its own.
    const noTerminal = availableSections({ byo: false, previews: true, terminal: false });
    expect(noTerminal.find((x) => x.id === "computer")!.pages.map((p) => p.label)).toEqual(["Computer", "Previews"]);
    expect(isPageAvailable("files", { byo: true, previews: true, terminal: true })).toBe(false);
    expect(resolvePage("files", { byo: false, previews: false, terminal: false })).toBe("chat");
  });

  it("gives a BYO agent Behaviour (Instructions), Computer (Computer: Connect) and Access only", () => {
    const s = availableSections(byo);
    expect(s.map((x) => x.id)).toEqual(["behaviour", "computer", "access"]);
    expect(s[0]!.pages.map((p) => p.id)).toEqual(["instructions"]);
    expect(s[1]!.pages.map((p) => [p.id, p.label])).toEqual([["connect", "Computer: Connect"]]);
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
});
