import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { WorkspaceSkills } from "@/components/skills/workspace-skills";
import { json } from "./support/api";
import { wrap } from "./support/render";
import { librarySource, me, menuItems, openMenu, routes, skill } from "./support/skills";

// RFE-020 and P5-SKILLS-3: a workspace's Skills page is the same list and panel as the agent's, with the workspace's
// own switch, how many agents use each skill, and the members' read-only view.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(),
}));

beforeEach(() => window.history.replaceState(null, "", "/o/acme/w/main/skills"));

const deploy = skill({
  id: "ws1",
  name: "deploy-checklist",
  tier: "workspace",
  used_by_agents: 6,
  overridden_in: [{ agent: { id: "a2", handle: "daneel", name: "Daneel" }, skill_id: "ag2" }],
  files: [{ path: "run.sh", size: 9, executable: true }],
});
const recipes = skill({ id: "ws2", name: "internal-comms", tier: "workspace", enabled: false, used_by_agents: 1, source: librarySource({ label: "Acme team recipes" }) });

const base = (role: string, items: unknown[], extra: Parameters<typeof routes>[0] = {}) => ({
  "GET /v1/me": () => json(200, me()),
  "GET /v1/resolve": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: { id: "w1", slug: "main", name: "Main" } }),
  "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", slug: "main", name: "Main", my_role: role }),
  "GET /v1/workspaces/w1/skills": () => json(200, { items, next_cursor: null }),
  "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [] }),
  "GET /v1/workspaces/w1/skill-library-updates": () => json(200, { items: [] }),
  ...extra,
});

const row = (name: string) => screen.getByTestId(`skill-row-${name}`);
const list = () => screen.findByRole("list", { name: "Skills" });
const open = async (name: string) => {
  await list();
  fireEvent.click(within(row(name)).getAllByRole("button")[0]!);
};

describe("workspace skills page", () => {
  it("is the one list: For <Workspace>, written here or from libraries, used by how many agents", async () => {
    const calls = routes(base("owner", [deploy, recipes]));
    wrap(<WorkspaceSkills />);
    await list();
    expect(screen.getByText("Skills every agent in Main can use")).toBeTruthy();
    expect(screen.getByText("For Main")).toBeTruthy();
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual(["All 2", "Written here 1", "From libraries 1", "Off 1"]);
    expect(within(row("deploy-checklist")).getByText("Used by 6 agents")).toBeTruthy();
    expect(within(row("internal-comms")).getByText("Used by 1 agent")).toBeTruthy();
    expect(within(row("deploy-checklist")).getByTestId("source-chip").textContent).toBe("Written here");
    expect(within(row("internal-comms")).getByTestId("source-chip").textContent).toBe("Library · Acme team recipes");
    expect(within(row("deploy-checklist")).getByRole("switch", { name: "Turn off for Main: Deploy checklist" })).toBeChecked();
    expect(document.body.textContent).not.toMatch(/home sync|organization's agents/i);
    // a workspace list asks for no agent's skills (no GET /v1/agents//skills)
    expect(calls.filter((c) => c.path.startsWith("/v1/agents/"))).toEqual([]);
  });

  it("the switch is the skill's own", async () => {
    const calls = routes(base("owner", [deploy], { "PATCH /v1/skills/ws1": () => json(200, { ...deploy, enabled: false }) }));
    wrap(<WorkspaceSkills />);
    await list();
    fireEvent.click(within(row("deploy-checklist")).getByRole("switch"));
    await waitFor(() => expect(calls.find((c) => c.method === "PATCH")?.body).toBe('{"enabled":false}'));
  });

  it("owner creates a workspace skill through the workspace route, from the header's Add skill", async () => {
    const calls = routes(base("owner", [], { "POST /v1/workspaces/w1/skills": () => json(201, skill({ id: "new", name: "release-notes", tier: "workspace" })) }));
    wrap(<WorkspaceSkills />);
    expect(await screen.findByText("No skills yet")).toBeTruthy();
    openMenu("Add skill");
    fireEvent.click(screen.getByRole("menuitem", { name: "Write a new skill" }));
    const form = await screen.findByRole("form", { name: "New skill" });
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "Release notes" } });
    fireEvent.change(within(form).getByLabelText("When should agents use it?"), { target: { value: "Write release notes" } });
    fireEvent.change(within(form).getByRole("textbox", { name: "Instructions" }), { target: { value: "steps" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    expect(JSON.parse(calls.find((c) => c.method === "POST")!.body)).toEqual({ name: "release-notes", title: "Release notes", description: "Write release notes", body: "steps", enabled: true });
  });

  it("keeps the precedence under a closed How skills combine", async () => {
    routes(base("owner", [deploy]));
    wrap(<WorkspaceSkills />);
    await list();
    const how = screen.getByRole("button", { name: "How skills combine" });
    expect(how).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByTestId("skills-combine")).toHaveTextContent("If both have a skill with the same name, the workspace's version is used.");
  });

  it("owner sees who overrides the skill, can edit it, and removes it from this workspace", async () => {
    routes(base("owner", [deploy]));
    wrap(<WorkspaceSkills />);
    await open("deploy-checklist");
    const p = await screen.findByRole("article", { name: "Deploy checklist" });
    expect(within(p).getByText(/Overridden by an agent skill of the same name in/)).toBeTruthy();
    expect(within(p).getByText("daneel")).toBeTruthy();
    expect(within(p).getByRole("button", { name: "Edit" })).toBeTruthy();
    expect(within(p).getByTestId("skill-usage").textContent).toBe("Used by 6 agents");
    // there is no chat to try it in from here
    expect(within(p).queryByRole("link", { name: "Try it in chat" })).toBeNull();
    openMenu("More actions for Deploy checklist");
    expect(menuItems()).toEqual(["Remove from this workspace…"]);
  });

  it("a member opens a skill read-only: the lock line, no fields, nothing to write", async () => {
    const calls = routes(base("member", [deploy]));
    wrap(<WorkspaceSkills />);
    await list();
    expect(screen.queryByRole("button", { name: "Add skill" })).toBeNull();
    expect(within(row("deploy-checklist")).getByRole("switch")).toBeDisabled();
    await open("deploy-checklist");
    const p = await screen.findByRole("article", { name: "Deploy checklist" });
    expect(within(p).getByTestId("skill-lock").textContent).toBe("Managed by the workspace’s owners.");
    expect(within(p).queryAllByRole("textbox")).toEqual([]);
    for (const name of ["Edit", "Save", "Delete"]) expect(within(p).queryByRole("button", { name })).toBeNull();
    expect(within(p).queryByRole("button", { name: /More actions/ })).toBeNull();
    expect(calls.some((c) => c.method !== "GET")).toBe(false);
  });
});
