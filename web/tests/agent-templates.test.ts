import { describe, expect, it } from "vitest";

import { handleFromName } from "@/components/agents/create-agent-dialog";
import { AGENT_TEMPLATES, BRIEF_MAX, DRAFT_MAX, fillTemplate, templateFor } from "@/lib/onboarding/templates";

describe("purpose templates (D-413)", () => {
  it("has the six chips in order, General assistant first", () => {
    expect(AGENT_TEMPLATES.map((t) => t.label)).toEqual([
      "General assistant", "Research", "Ops helper", "Calendar secretary", "Writer", "A blank agent",
    ]);
  });
  it("gives every template all its fields and fits the API's limits", () => {
    for (const t of AGENT_TEMPLATES) {
      expect(t.id, t.label).toBeTruthy();
      expect(t.firstMessage.length, t.label).toBeGreaterThan(0);
      expect(fillTemplate(t.firstMessage, { name: "A".repeat(80) }).length, t.label).toBeLessThanOrEqual(DRAFT_MAX);
      expect(t.instructions.length, t.label).toBeLessThanOrEqual(BRIEF_MAX);
      if (t.id === "blank") {
        expect(t.instructions).toBe("");
        continue;
      }
      expect(t.nameSuggestion, t.label).not.toBe("");
      const lines = t.instructions.split("\n");
      expect(lines.length, t.label).toBeGreaterThanOrEqual(6);
      expect(lines.length, t.label).toBeLessThanOrEqual(10);
      expect(t.instructions.toLowerCase(), t.label).toMatch(/ask(ing)? (first|before)/);
    }
  });
  it("sets instructions only: no template carries access", () => {
    for (const t of AGENT_TEMPLATES) expect(Object.keys(t).sort()).not.toContain("grants");
  });
  it("templateFor falls back to the General assistant; fillTemplate fills name and person", () => {
    expect(templateFor("nope").id).toBe("general");
    expect(templateFor("ops").label).toBe("Ops helper");
    expect(fillTemplate("You are {name}, for {person}.", { name: "Rex", person: "Bob" })).toBe("You are Rex, for Bob.");
    expect(fillTemplate("{name} / {person}", { name: " ", person: null })).toBe("your agent / the person you work for");
  });
  it("keeps the optional schedule as data for the schedule form", () => {
    for (const t of AGENT_TEMPLATES) {
      if (t.schedule) {
        expect(t.schedule.cron.split(" ")).toHaveLength(5);
        expect(t.schedule.prompt.length).toBeGreaterThan(0);
      }
    }
  });
});

describe("handleFromName", () => {
  it("makes a handle the API accepts", () => {
    expect(handleFromName("Rex")).toBe("rex");
    expect(handleFromName("Rex the Ops helper")).toBe("rex-the-ops-helper");
    expect(handleFromName("Zoë  Å")).toBe("zoe-a");
    expect(handleFromName("42 bots")).toBe("bots");
    expect(handleFromName("日本語")).toBe("");
    expect(handleFromName("a".repeat(60)).length).toBe(40);
  });
});
