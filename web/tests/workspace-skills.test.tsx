import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { EffectiveSkillsView, SkillsEditor } from "@/components/skills/skills-view";
import { WorkspaceSkills } from "@/components/skills/workspace-skills";
import type { Skill } from "@/lib/api/skills";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(),
}));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${decodeURIComponent(url.pathname)}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const T = "2026-01-01T00:00:00Z";
function skill(over: Partial<Skill> & Pick<Skill, "id" | "name" | "tier">): Skill {
  return {
    scope: over.tier === "org" ? "organization" : over.tier,
    agent_id: over.tier === "agent" ? "a1" : null,
    workspace_id: over.tier === "workspace" ? "w1" : null,
    org_id: "o1",
    description: `${over.name} description`,
    body: `${over.name} body`,
    enabled: true,
    files: [],
    total_size: 10,
    overridden_in: null,
    created_at: T,
    updated_at: T,
    ...over,
  } as Skill;
}

const deploy = skill({
  id: "ws1",
  name: "deploy-checklist",
  tier: "workspace",
  overridden_in: [{ agent: { id: "a2", handle: "daneel", name: "Daneel" }, skill_id: "ag2" }],
  files: [{ path: "run.sh", size: 9, executable: true }],
});

describe("workspace skills page", () => {
  const resolve = () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: { id: "w1", slug: "main", name: "Main" } });
  const ws = (role: string) => () => json(200, { id: "w1", org_id: "o1", slug: "main", name: "Main", my_role: role });

  it("owner creates a workspace skill through the workspace route", async () => {
    let created: unknown = null;
    mockFetch({
      "GET /v1/resolve": resolve,
      "GET /v1/workspaces/w1": ws("owner"),
      "GET /v1/workspaces/w1/skills": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/workspaces/w1/skills": async (r) => {
        created = await r.json();
        return json(201, skill({ id: "new", name: "release-notes", tier: "workspace" }));
      },
    });
    wrap(<WorkspaceSkills />);
    fireEvent.click(await screen.findByRole("button", { name: "New skill" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "release-notes" } });
    fireEvent.change(screen.getByLabelText("Description"), { target: { value: "Write release notes" } });
    fireEvent.change(screen.getByLabelText("SKILL.md"), { target: { value: "steps" } });
    fireEvent.click(screen.getByRole("button", { name: "Create skill" }));
    await waitFor(() => expect(created).toEqual({ name: "release-notes", description: "Write release notes", body: "steps" }));
  });

  it("owner sees who overrides the skill and can edit", async () => {
    mockFetch({
      "GET /v1/resolve": resolve,
      "GET /v1/workspaces/w1": ws("owner"),
      "GET /v1/workspaces/w1/skills": () => json(200, { items: [deploy], next_cursor: null }),
    });
    wrap(<WorkspaceSkills />);
    fireEvent.click(await screen.findByRole("button", { name: /deploy-checklist/ }));
    expect(await screen.findByText(/Overridden by an agent skill of the same name in/)).toBeTruthy();
    expect(screen.getByText("daneel")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Delete" })).toBeTruthy();
  });

  it("member opens a workspace skill read-only: no create, save, delete or file controls", async () => {
    const fn = mockFetch({
      "GET /v1/resolve": resolve,
      "GET /v1/workspaces/w1": ws("member"),
      "GET /v1/workspaces/w1/skills": () => json(200, { items: [deploy], next_cursor: null }),
    });
    wrap(<WorkspaceSkills />);
    fireEvent.click(await screen.findByRole("button", { name: /deploy-checklist/ }));
    expect(await screen.findByText(/Only workspace owners can change them/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "New skill" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Save file" })).toBeNull();
    expect(screen.queryByLabelText("Replace run.sh")).toBeNull();
    expect((screen.getByLabelText("SKILL.md") as HTMLTextAreaElement).readOnly).toBe(true);
    expect(fn.mock.calls.some(([r]) => (r as Request).method !== "GET")).toBe(false);
  });
});

describe("agent Skills tab: effective set", () => {
  const mine = skill({ id: "ag1", name: "triage", tier: "agent" });
  const org = skill({ id: "org1", name: "style-guide", tier: "org" });
  const orgDeploy = skill({ id: "org2", name: "deploy-checklist", tier: "org" });
  const effective = {
    items: [
      { skill: deploy, tier: "workspace", enabled: true, shadows: [{ id: "org2", tier: "org" }] },
      { skill: mine, tier: "agent", enabled: true, shadows: [] },
      { skill: org, tier: "org", enabled: true, shadows: [] },
    ],
    shadowed: [
      { skill: orgDeploy, tier: "org", enabled: true, shadowed_by: { id: "ws1", tier: "workspace" } },
      { skill: skill({ id: "off", name: "old-one", tier: "workspace", enabled: false }), tier: "workspace", enabled: false, shadowed_by: null },
    ],
  };

  it("shows tier badges, what a skill overrides, what is shadowed, and opens read-only", async () => {
    mockFetch({ "GET /v1/agents/a1/skills/effective": () => json(200, effective) });
    wrap(<EffectiveSkillsView agentId="a1" />);
    await screen.findByRole("button", { name: "Open style-guide (Organization)" });
    const received = screen.getByRole("list", { name: "Received skills" });
    const rows = within(received).getAllByRole("listitem");
    expect(rows).toHaveLength(3);
    expect(within(rows[0]!).getByText("Workspace")).toBeTruthy();
    expect(within(rows[0]!).getByText("Overrides organization skill")).toBeTruthy();
    expect(within(rows[1]!).getByText("Agent")).toBeTruthy();
    expect(within(rows[2]!).getByText("Organization")).toBeTruthy();
    const notApplied = screen.getByRole("list", { name: "Skills not applied" });
    expect(within(notApplied).getByText("Shadowed by the workspace skill")).toBeTruthy();
    expect(within(notApplied).getByText("Disabled")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Open style-guide (Organization)" }));
    expect((await screen.findByLabelText("SKILL.md") as HTMLTextAreaElement).readOnly).toBe(true);
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("the agent's own list stays editable", async () => {
    mockFetch({ "GET /v1/agents/a1/skills": () => json(200, { items: [mine], next_cursor: null }) });
    wrap(<SkillsEditor scope={{ kind: "agent", id: "a1" }} />);
    fireEvent.click(await screen.findByRole("button", { name: /triage/ }));
    expect(await screen.findByRole("button", { name: "Save" })).toBeTruthy();
  });
});
