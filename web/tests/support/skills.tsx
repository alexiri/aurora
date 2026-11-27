import { fireEvent, screen } from "@testing-library/react";
import { vi } from "vitest";

import { SkillsTab } from "@/components/skills/skills-tab";
import { json, type Handler } from "./api";

// (P5-SKILLS-3) Skill fixtures shaped like the review's data (docs/design/skills-memory), and the agent's Skills page
// with its routes.

export const T = "2026-11-20T10:00:00Z";
export const SHA_A = "e4bb5d9e4bb5d9e4bb5d9e4bb5d9e4bb5d9e4bb5";

export type SkillFixture = Record<string, unknown> & { id: string; name: string; tier: string; enabled: boolean };

export const skill = (over: Record<string, unknown> = {}): SkillFixture => {
  const tier = (over.tier as string) ?? "agent";
  const name = (over.name as string) ?? "triage-alerts";
  return {
    id: name,
    org_id: "o1",
    agent_id: tier === "agent" ? "a1" : null,
    workspace_id: tier === "workspace" ? "w1" : null,
    scope: tier === "org" ? "organization" : tier,
    tier,
    name,
    title: null,
    description: `Do the ${name} thing.`,
    body: `# ${name}\n\nSteps.`,
    enabled: true,
    files: [] as { path: string; size: number; executable: boolean }[],
    total_size: 20,
    overridden_in: null,
    used_by_agents: tier === "agent" ? null : 3,
    source: null,
    created_at: T,
    updated_at: T,
    ...over,
  };
};

export const librarySource = (over: Record<string, unknown> = {}) => ({
  kind: "library",
  library_id: "l1",
  label: "Anthropic skills",
  repo: "github.com/anthropics/skills",
  path: "pdf",
  commit_sha: SHA_A,
  pinned_at: T,
  detached: false,
  update_pending: false,
  ...over,
});

/** An entry of the effective set. */
export const applied = (s: ReturnType<typeof skill>, over: Record<string, unknown> = {}) => ({
  skill: s,
  tier: s.tier,
  enabled: true,
  shadows: [] as unknown[],
  off_for_agent: false,
  usage: { week: 0, last: null },
  ...over,
});

/** An entry of `shadowed`. */
export const notApplied = (s: ReturnType<typeof skill>, over: Record<string, unknown> = {}) => ({
  skill: s,
  tier: s.tier,
  enabled: s.enabled,
  shadowed_by: null,
  off_for_agent: false,
  reason: "replaced",
  usage: { week: 0, last: null },
  ...over,
});

export const me = (orgRole: "owner" | "admin" | "member" = "member", wsRole: "owner" | "member" = "owner") => ({
  user: { id: "u1", display_name: "Bob Builder", email: "bob@example.test" },
  memberships: [{ org: { id: "o1", slug: "acme", name: "Acme" }, role: orgRole, status: "active", workspaces: [{ id: "w1", slug: "main", name: "Main", my_role: wsRole, personal: false }] }],
});

/** `fetch` answering `METHOD /decoded/path`; also records every call. */
export function routes(table: Record<string, Handler>) {
  const calls: { method: string; path: string; body: string; query: string }[] = [];
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = typeof input === "string" || input instanceof URL ? new Request(new URL(input, "http://localhost")) : input;
    const url = new URL(req.url, "http://localhost");
    const path = decodeURIComponent(url.pathname);
    calls.push({ method: req.method, path, body: req.method === "GET" ? "" : await req.clone().text(), query: url.search });
    const h = table[`${req.method} ${path}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return calls;
}

export const AgentSkills = ({ readOnly = false }: { readOnly?: boolean }) => (
  <SkillsTab agentId="a1" orgId="o1" wsId="w1" handle="daneel" agentName="Daneel" readOnly={readOnly} />
);

export const effective = (items: unknown[], shadowed: unknown[] = []) => () => json(200, { items, shadowed });

/** Radix opens a dropdown on keydown Enter (jsdom has no pointer capture). */
export const openMenu = (name: string) => fireEvent.keyDown(screen.getByRole("button", { name }), { key: "Enter" });
export const menuItems = () => screen.getAllByRole("menuitem").map((i) => i.textContent);
