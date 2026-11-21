import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { EffectiveSkillsView, SkillsEditor } from "@/components/skills/skills-view";
import { baseSkillName, newSkillName, skillDisplayName } from "@/lib/skill-name";
import { err, json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

vi.mock("next/navigation", () => ({ useParams: () => ({ org: "acme", ws: "main" }), useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));

describe("skill folder names are made by Aurora (behaviour-2)", () => {
  it("makes kebab-case ASCII from the name, at most 64 characters", () => {
    expect(baseSkillName("Weekly report")).toBe("weekly-report");
    expect(baseSkillName("Crème brûlée!")).toBe("creme-brulee");
    expect(baseSkillName("日本語")).toBe("skill");
    expect(baseSkillName("x".repeat(100)).length).toBe(64);
  });
  it("never starts with the reserved aurora-", () => {
    expect(baseSkillName("Aurora tips")).toBe("my-aurora-tips");
    expect(baseSkillName("aurora")).toBe("my-aurora");
    expect(baseSkillName("Auroral light")).toBe("auroral-light");
  });
  it("is unique by a numeric suffix, and a retry moves on", () => {
    expect(newSkillName("Weekly report", ["weekly-report"])).toBe("weekly-report-2");
    expect(newSkillName("Weekly report", ["weekly-report", "weekly-report-2"])).toBe("weekly-report-3");
    expect(newSkillName("Weekly report", [], 1)).toBe("weekly-report-2");
    expect(newSkillName("x".repeat(100), ["x".repeat(64)]).length).toBeLessThanOrEqual(64);
  });
  it("shows the stored name as words", () => {
    expect(skillDisplayName("weekly-report")).toBe("Weekly report");
  });
});

const skill = (over: Record<string, unknown> = {}) => ({
  id: "s1", org_id: "o1", name: "weekly-report", description: "someone asks for a weekly status report", body: "List wins.",
  tier: "agent", enabled: true, files: [], total_size: 10, created_at: "2026-11-20T10:00:00Z", updated_at: "2026-11-20T10:00:00Z", ...over,
});

describe("the skill form", () => {
  it("creates by name and 'Use it when…', with no SKILL.md, file path or Executable on the form", async () => {
    const posts: Record<string, unknown>[] = [];
    mockFetch({
      "GET /v1/agents/a1/skills": () => json(200, { items: [skill({ id: "x", name: "other" })], next_cursor: null }),
      "POST /v1/agents/a1/skills": async (r) => {
        const b = (await r.json()) as Record<string, unknown>;
        posts.push(b);
        return json(201, skill({ name: b.name }));
      },
    });
    wrap(<SkillsEditor scope={{ kind: "agent", id: "a1" }} agentName="Dani" />);
    fireEvent.click(await screen.findByRole("button", { name: "New skill" }));
    for (const gone of ["SKILL.md", "File path", "Executable", "Description"]) expect(screen.queryByLabelText(gone)).toBeNull();
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Weekly report" } });
    fireEvent.change(screen.getByLabelText("Use it when…"), { target: { value: "someone asks for a weekly status report" } });
    fireEvent.change(screen.getByLabelText("Instructions"), { target: { value: "List wins." } });
    // (behaviour-2) A new skill has its switch too, on by default, named for the agent.
    const sw = screen.getByRole("switch", { name: "On for Dani" });
    expect(sw).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(posts).toHaveLength(1));
    expect(posts[0]).toEqual({ name: "weekly-report", description: "someone asks for a weekly status report", body: "List wins.", enabled: true });
  });

  it("creates a skill that is off when the switch is turned off", async () => {
    const posts: Record<string, unknown>[] = [];
    mockFetch({
      "GET /v1/agents/a1/skills": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/agents/a1/skills": async (r) => {
        const b = (await r.json()) as Record<string, unknown>;
        posts.push(b);
        return json(201, skill({ name: b.name, enabled: false }));
      },
    });
    wrap(<SkillsEditor scope={{ kind: "agent", id: "a1" }} agentName="Dani" />);
    fireEvent.click(await screen.findByRole("button", { name: "New skill" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Weekly report" } });
    fireEvent.change(screen.getByLabelText("Use it when…"), { target: { value: "asked" } });
    fireEvent.click(screen.getByRole("switch", { name: "On for Dani" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(posts).toHaveLength(1));
    expect(posts[0]).toMatchObject({ enabled: false });
  });

  it("retries once with the next suffix when the folder name is taken", async () => {
    const names: string[] = [];
    mockFetch({
      "GET /v1/agents/a1/skills": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/agents/a1/skills": async (r) => {
        const b = (await r.json()) as { name: string };
        names.push(b.name);
        return names.length === 1 ? err(409, "conflict", "A skill with this name already exists in this scope.") : json(201, skill({ name: b.name }));
      },
    });
    wrap(<SkillsEditor scope={{ kind: "agent", id: "a1" }} />);
    fireEvent.click(await screen.findByRole("button", { name: "New skill" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Weekly report" } });
    fireEvent.change(screen.getByLabelText("Use it when…"), { target: { value: "asked" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(names).toEqual(["weekly-report", "weekly-report-2"]));
  });

  it("lists by display name and shows 'On for Dani' with Advanced: files and scripts only at Expert", async () => {
    mockFetch({ "GET /v1/agents/a1/skills": () => json(200, { items: [skill()], next_cursor: null }) });
    const { unmount } = wrap(<SkillsEditor scope={{ kind: "agent", id: "a1" }} agentName="Dani" />);
    // The row's second line reads "Use it when …" (mockup behaviour-2).
    expect(await screen.findByText("Use it when someone asks for a weekly status report")).toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: /Weekly report/ }));
    expect(screen.getByRole("switch", { name: "On for Dani" })).toBeChecked();
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeInTheDocument();
    // The Advanced fold sits inside the card, above Save, and Save still belongs to the skill's own form.
    const fold = screen.getByText("Advanced: files and scripts");
    expect(fold.compareDocumentPosition(save) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(fold.closest("form")).toBeNull();
    expect(save.getAttribute("form")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Delete" })).toBeInTheDocument();
    expect(screen.getByText("Advanced: files and scripts")).toBeInTheDocument();
    expect(screen.getByLabelText("File path")).toBeInTheDocument();
    expect(screen.getByText("weekly-report")).toBeInTheDocument(); // the folder name, in the fold
    unmount();
    mockFetch({ "GET /v1/agents/a1/skills": () => json(200, { items: [skill()], next_cursor: null }) });
    wrap(<SkillsEditor scope={{ kind: "agent", id: "a1" }} agentName="Dani" />, { moreControls: false });
    fireEvent.click(await screen.findByRole("button", { name: /Weekly report/ }));
    expect(screen.queryByLabelText("File path")).toBeNull();
    expect(screen.queryByLabelText("Executable")).toBeNull();
    expect(screen.getByText("Advanced: files and scripts")).toBeInTheDocument(); // the collapsed line
  });

  it("moves the precedence paragraph to Expert on 'Skills this agent receives'", async () => {
    const eff = { items: [{ skill: skill(), tier: "agent", enabled: true, shadows: [] }], shadowed: [] };
    mockFetch({ "GET /v1/agents/a1/skills/effective": () => json(200, eff) });
    const { unmount } = wrap(<EffectiveSkillsView agentId="a1" />);
    expect(await screen.findByText(/One skill per name\./)).toBeInTheDocument();
    unmount();
    mockFetch({ "GET /v1/agents/a1/skills/effective": () => json(200, eff) });
    wrap(<EffectiveSkillsView agentId="a1" />, { moreControls: false });
    await screen.findByText("Skills this agent receives");
    expect(screen.queryByText(/One skill per name\./)).toBeNull();
  });
});
