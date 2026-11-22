import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { EffectiveSkillsView, SkillsEditor } from "@/components/skills/skills-view";
import { WorkspaceSkills } from "@/components/skills/workspace-skills";
import type { Skill } from "@/lib/api/skills";
import { wrap } from "./support/render";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(),
}));

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
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Release notes" } });
    fireEvent.change(screen.getByLabelText("Use it when…"), { target: { value: "Write release notes" } });
    fireEvent.change(screen.getByLabelText("Instructions"), { target: { value: "steps" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(created).toEqual({ name: "release-notes", description: "Write release notes", body: "steps", enabled: true }));
  });

  it("opens with what a skill is, keeps the precedence in a closed fold, and never says home sync (workspace-10)", async () => {
    mockFetch({
      "GET /v1/resolve": resolve,
      "GET /v1/workspaces/w1": ws("owner"),
      "GET /v1/workspaces/w1/skills": () => json(200, { items: [], next_cursor: null }),
    });
    wrap(<WorkspaceSkills />);
    expect(await screen.findByRole("heading", { name: "Skills" })).toBeTruthy();
    expect(screen.getByText(/A skill is a set of instructions that teaches an agent how to do one job your way/)).toBeTruthy();
    const fold = screen.getByTestId("skills-combine");
    expect(fold).not.toHaveAttribute("open");
    expect(fold).toHaveTextContent("If both have a skill with the same name, the workspace's version is used.");
    expect(document.body.textContent).not.toMatch(/home sync/i);
  });

  it("owner sees who overrides the skill and can edit", async () => {
    mockFetch({
      "GET /v1/resolve": resolve,
      "GET /v1/workspaces/w1": ws("owner"),
      "GET /v1/workspaces/w1/skills": () => json(200, { items: [deploy], next_cursor: null }),
    });
    wrap(<WorkspaceSkills />);
    fireEvent.click(await screen.findByRole("button", { name: /Deploy checklist/ }));
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
    fireEvent.click(await screen.findByRole("button", { name: /Deploy checklist/ }));
    expect(await screen.findByText(/Only workspace owners can change them/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "New skill" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Save file" })).toBeNull();
    expect(screen.queryByLabelText("Replace run.sh")).toBeNull();
    expect((screen.getByLabelText("Instructions") as HTMLTextAreaElement).readOnly).toBe(true);
    expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method !== "GET")).toBe(false);
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
    expect((await screen.findByLabelText("Instructions") as HTMLTextAreaElement).readOnly).toBe(true);
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("lists app skills read-only under From apps, with the app and version, and names the winner of a shadowed one", async () => {
    const appBody = (name: string, files: Array<{ path: string; size: number; executable: boolean }> = []) => ({
      id: null,
      org_id: null,
      agent_id: null,
      workspace_id: null,
      created_at: null,
      updated_at: null,
      tier: "app",
      scope: "app",
      name,
      description: `${name} description`,
      body: `# ${name}\n<img src=x onerror="window.__pwned=1">`,
      enabled: true,
      files,
      total_size: 10,
      overridden_in: null,
    });
    const app = { app_id: "git", name: "Git", version: "1.2.0" };
    const withApps = {
      items: [
        ...effective.items,
        { skill: appBody("git-api", [{ path: "scripts/run.sh", size: 12, executable: true }]), tier: "app", enabled: true, shadows: [], app },
      ],
      shadowed: [
        ...effective.shadowed,
        { skill: appBody("triage"), tier: "app", enabled: true, shadowed_by: { id: "ag1", tier: "agent", app_id: null }, app },
      ],
    };
    mockFetch({ "GET /v1/agents/a1/skills/effective": () => json(200, withApps) });
    const { container } = wrap(<EffectiveSkillsView agentId="a1" />);
    const apps = await screen.findByRole("list", { name: "Skills from apps" });
    expect(screen.getByRole("heading", { name: "From apps" })).toBeTruthy();
    const row = within(apps).getByTestId("app-skill-git-api");
    expect(within(row).getByText("From the Git app · 1.2.0")).toBeTruthy();
    expect(within(row).getByText("Script")).toBeTruthy();
    // Read-only: no button of any kind in the row except the fold's summary; no edit, disable, delete.
    expect(within(row).queryByRole("button")).toBeNull();
    expect(within(row).queryByRole("switch")).toBeNull();
    expect(within(row).queryByRole("checkbox")).toBeNull();
    // The body is text, not markup.
    expect(within(row).getByRole("region", { name: "Instructions of git-api" })).toHaveTextContent('<img src=x onerror="window.__pwned=1">');
    expect(container.querySelector("img")).toBeNull();
    // A shadowed app skill names its winner, and has no controls either.
    const notApplied = screen.getByRole("list", { name: "Skills not applied" });
    const shadowedRow = within(notApplied).getByTestId("app-skill-triage");
    expect(within(shadowedRow).getByText("Shadowed by the agent skill")).toBeTruthy();
    expect(within(shadowedRow).queryByRole("button")).toBeNull();
    // The database tiers are unchanged and still open to read.
    expect(within(screen.getByRole("list", { name: "Received skills" })).getAllByRole("listitem")).toHaveLength(3);
  });

  it("an app skill that loses to another app names that app", async () => {
    const body = { id: null, org_id: null, agent_id: null, workspace_id: null, created_at: null, updated_at: null, tier: "app", scope: "app", name: "jira-x", description: "d", body: "b", enabled: true, files: [], total_size: 1, overridden_in: null };
    const jira = { app_id: "jira", name: "Jira", version: "1.0.0" };
    const jiraDc = { app_id: "jira-dc", name: "Jira Data Center", version: "2.0.0" };
    mockFetch({
      "GET /v1/agents/a1/skills/effective": () =>
        json(200, {
          items: [{ skill: body, tier: "app", enabled: true, shadows: [{ tier: "app", id: null, app_id: "jira" }], app: jiraDc }],
          shadowed: [{ skill: body, tier: "app", enabled: true, shadowed_by: { tier: "app", id: null, app_id: "jira-dc" }, app: jira }],
        }),
    });
    wrap(<EffectiveSkillsView agentId="a1" />);
    const notApplied = await screen.findByRole("list", { name: "Skills not applied" });
    expect(within(notApplied).getByText("Shadowed by the Jira Data Center app's skill")).toBeTruthy();
  });

  it("the agent's own list stays editable", async () => {
    mockFetch({ "GET /v1/agents/a1/skills": () => json(200, { items: [mine], next_cursor: null }) });
    wrap(<SkillsEditor scope={{ kind: "agent", id: "a1" }} />);
    fireEvent.click(await screen.findByRole("button", { name: /triage/ }));
    expect(await screen.findByRole("button", { name: "Save" })).toBeTruthy();
  });

  it("a workspace list asks for no agent's skills (no GET /v1/agents//skills, P5-POLISH-11 pass 2)", async () => {
    const fn = mockFetch({ "GET /v1/workspaces/w1/skills": () => json(200, { items: [deploy], next_cursor: null }) });
    wrap(<SkillsEditor scope={{ kind: "workspace", id: "w1" }} />);
    expect(await screen.findByRole("button", { name: /Deploy checklist/ })).toBeTruthy();
    const paths = fn.mock.calls.map(([input]) => decodeURIComponent(new URL((input as Request).url, "http://localhost").pathname));
    expect(paths.filter((p) => p.startsWith("/v1/agents/"))).toEqual([]);
  });
});
