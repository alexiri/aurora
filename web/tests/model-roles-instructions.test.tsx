import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { InstructionsTab } from "@/components/agents/instructions-tab";
import type { Agent } from "@/lib/api/queries";
import { wrap as wrapWith } from "./support/render";

// The model roles, effort and agent program are Expert (D-406): these tests run with "Show more controls" on.
const wrap = (ui: React.ReactNode) => wrapWith(ui, { moreControls: true });
import { json } from "./support/api";

vi.mock("next/link", () => ({ default: ({ children }: { children: React.ReactNode }) => <a>{children}</a> }));

const agent = (over: Partial<Agent> = {}): Agent =>
  ({
    id: "a1", org_id: "o1", workspace_id: "w1", handle: "bot", name: "Bot", kind: "hosted",
    is_default: false, harness: "claude-code", model_provider: "litellm", model_endpoint_id: "gw", model_id: "claude-sonnet",
    effort: null, proactive_messaging: "ask", brief: "", avatar_url: null, subagent_model: "inherit", model_roles: null,
    created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
    ...over,
  }) as Agent;

const models = {
  items: [
    { endpoint_id: "gw", model_id: "claude-opus", provider: "litellm", dialects: [] },
    { endpoint_id: "gw", model_id: "claude-sonnet", provider: "litellm", dialects: [] },
    { endpoint_id: "other", model_id: "elsewhere", provider: "litellm", dialects: [] },
  ],
};
const harnesses = (roles: string[]) => ({
  default: "claude-code",
  items: [
    { id: "claude-code", display_name: "Claude Code", available: true, dialects: [], model_roles: ["large", "medium", "small"] },
    { id: "codex", display_name: "Codex", available: true, dialects: [], model_roles: roles },
  ],
});
const view = (model_id: string | null, source: string, warning: string | null = null) => ({ model_id, source, warning });
const readiness = (over: Record<string, unknown> = {}) => ({
  keys: [],
  override_dropped: false,
  default: {
    endpoint_id: "gw", model_id: "claude-sonnet", provider: "litellm", source: "override",
    roles: { large: view("claude-opus", "endpoint"), medium: view(null, "none", "role_unset"), small: view(null, "none", "role_unset") },
    roles_dropped: [], subagent_model: "inherit", subagent_warning: null,
    ...over,
  },
});

const calls: { method: string; path: string; body?: unknown }[] = [];
function mockFetch(routes: Record<string, () => Response>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const method = req?.method ?? init?.method ?? "GET";
      const body = req && method !== "GET" ? await req.clone().json().catch(() => undefined) : undefined;
      calls.push({ method, path: url.pathname, body });
      const h = routes[`${method} ${url.pathname}`];
      return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}
const base = (roles: string[] = ["large", "medium", "small"]) => ({
  "GET /v1/agents/a1/models": () => json(200, models),
  "GET /v1/harnesses": () => json(200, harnesses(roles)),
  "GET /v1/agents/a1/model-readiness": () => json(200, readiness()),
});

beforeEach(() => {
  calls.length = 0;
});
const patches = () => calls.filter((c) => c.method === "PATCH");
/** A role select once the endpoint's models have loaded into it. */
async function roleSelect(name: string): Promise<HTMLSelectElement> {
  const el = (await screen.findByLabelText(name)) as HTMLSelectElement;
  await waitFor(() => expect(within(el).getByRole("option", { name: "claude-opus" })).toBeInTheDocument());
  return el;
}

describe("Instructions: model roles (P5-MODELS-3)", () => {
  it("labels the default model, lists the endpoint's models per role with the claude-code alias", async () => {
    mockFetch(base());
    wrap(<InstructionsTab agent={agent()} wsId="w1" />);
    expect(screen.getByLabelText("Model")).toBeInTheDocument();
    expect(screen.getByText(/It may use other allowed models on the same endpoint/)).toBeInTheDocument();
    const large = await roleSelect("Large (opus)");
    expect(screen.getByLabelText("Medium (sonnet)")).toBeInTheDocument();
    expect(screen.getByLabelText("Small (haiku)")).toBeInTheDocument();
    const names = within(large).getAllByRole("option").map((o) => o.textContent);
    expect(names).toContain("claude-opus");
    expect(names).not.toContain("elsewhere");
    await waitFor(() => expect(within(large).getByRole("option", { name: "Use the endpoint's setting (claude-opus)" })).toBeInTheDocument());
    expect(within(screen.getByLabelText("Small (haiku)")).getByRole("option", { name: "Use the endpoint's setting (none)" })).toBeInTheDocument();
    // (P5-POLISH-6, D-385) The role warnings are not on this page; the endpoint shows them.
    expect(screen.queryAllByTestId("role-warning")).toHaveLength(0);
  });

  it("saves roles with the default endpoint and the subagent default; clearing them sends null", async () => {
    mockFetch({ ...base(), "PATCH /v1/agents/a1": () => json(200, agent()) });
    wrap(<InstructionsTab agent={agent()} wsId="w1" />);
    fireEvent.change(await roleSelect("Small (haiku)"), { target: { value: "claude-sonnet" } });
    fireEvent.change(screen.getByLabelText("Subagents use"), { target: { value: "small" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(patches()).toHaveLength(1));
    expect(patches()[0]!.body).toMatchObject({ model_roles: { endpoint_id: "gw", small: "claude-sonnet" }, subagent_model: "small" });
  });

  it("clears stored roles with null and leaves an untouched group out of the request", async () => {
    mockFetch({ ...base(), "PATCH /v1/agents/a1": () => json(200, agent()) });
    const stored = agent({ model_roles: { endpoint_id: "gw", small: "claude-sonnet" }, subagent_model: "small" });
    wrap(<InstructionsTab agent={stored} wsId="w1" />);
    // Only the instructions change: the roles and the subagent model stay out of the request.
    fireEvent.change(await screen.findByLabelText("Instructions"), { target: { value: "Changed" } });
    fireEvent.click(await screen.findByRole("button", { name: "Save" }));
    await waitFor(() => expect(patches()).toHaveLength(1));
    expect(patches()[0]!.body).not.toHaveProperty("model_roles");
    expect(patches()[0]!.body).not.toHaveProperty("subagent_model");
    fireEvent.change(screen.getByLabelText("Small (haiku)"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(patches()).toHaveLength(2));
    expect(patches()[1]!.body).toMatchObject({ model_roles: null });
  });

  it("shows the Codex note instead of the group when the harness maps no roles", async () => {
    mockFetch(base([]));
    wrap(<InstructionsTab agent={agent({ harness: "codex" })} wsId="w1" />);
    expect(await screen.findByTestId("roles-not-used")).toHaveTextContent("Not used by Codex");
    expect(screen.queryByLabelText("Subagents use")).toBeNull();
  });

  it("lists every harness by its display name and says 'Not used by <name>' for each role it does not map (C-1016)", async () => {
    const items = [
      { id: "claude-code", display_name: "Claude Code", available: true, dialects: [], model_roles: ["large", "medium", "small"] },
      { id: "codex", display_name: "Codex", available: true, dialects: [], model_roles: [] },
      { id: "pi", display_name: "Pi", available: true, dialects: [], model_roles: [] },
      { id: "goose", display_name: "goose", available: true, dialects: [], model_roles: ["large", "medium", "small"] },
      { id: "opencode", display_name: "OpenCode", available: true, dialects: [], model_roles: ["small"] },
    ];
    mockFetch({ ...base(), "GET /v1/harnesses": () => json(200, { default: "claude-code", items }) });
    wrap(<InstructionsTab agent={agent({ harness: "opencode" })} wsId="w1" />);
    await waitFor(() => expect(screen.getByLabelText("Harness").tagName).toBe("SELECT"));
    const picker = screen.getByLabelText("Harness");
    expect(within(picker).getAllByRole("option").map((o) => o.textContent)).toEqual(["Claude Code", "Codex", "Pi", "goose", "OpenCode"]);
    // OpenCode (as configured here) maps `small` only: the other two roles say so and cannot be set.
    const large = (await screen.findByLabelText("Large")) as HTMLSelectElement;
    await waitFor(() => expect(large).toBeDisabled());
    expect(screen.getAllByText("Not used by OpenCode.")).toHaveLength(2);
    expect(screen.getByLabelText("Small")).not.toBeDisabled();
    const subagent = screen.getByLabelText("Subagents use");
    expect(within(subagent).getAllByRole("option").map((o) => o.textContent)).toEqual(["Same as the default", "Small"]);
    // A harness that maps none still shows the group note with its own name.
    cleanup();
    wrap(<InstructionsTab agent={agent({ harness: "pi" })} wsId="w1" />);
    expect(await screen.findByTestId("roles-not-used")).toHaveTextContent("Not used by Pi");
  });

  it("puts model_role_endpoint_mismatch and llm_model_not_allowed on their fields", async () => {
    mockFetch({
      ...base(),
      "PATCH /v1/agents/a1": () =>
        json(422, { error: { code: "llm_model_not_allowed", message: "no", details: { field: "model_roles.large", model: "claude-opus", allowed_models: [] } } }),
    });
    wrap(<InstructionsTab agent={agent()} wsId="w1" />);
    fireEvent.change(await roleSelect("Large (opus)"), { target: { value: "claude-opus" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const field = await screen.findByLabelText("Large (opus)");
    await waitFor(() => expect(field).toHaveAttribute("aria-invalid", "true"));
    expect(screen.getByText("claude-opus is not allowed on this endpoint.")).toBeInTheDocument();

    cleanup();
    mockFetch({
      ...base(),
      "PATCH /v1/agents/a1": () =>
        json(422, { error: { code: "model_role_endpoint_mismatch", message: "no", details: { endpoint_id: "x", default_endpoint_id: "gw" } } }),
    });
    wrap(<InstructionsTab agent={agent()} wsId="w1" />);
    fireEvent.change(await roleSelect("Large (opus)"), { target: { value: "claude-opus" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/belong to another endpoint/)).toBeInTheDocument();
  });

  it("shows no role warning on the page, even when readiness reports a drop or a not-allowed role (D-385)", async () => {
    mockFetch({
      ...base(),
      "GET /v1/agents/a1/model-readiness": () =>
        json(200, readiness({
          roles: { large: view("claude-opus", "agent", "role_not_allowed"), medium: view("claude-sonnet", "endpoint"), small: view("claude-sonnet", "endpoint") },
          roles_dropped: ["small"],
          subagent_model: "medium",
        })),
    });
    wrap(<InstructionsTab agent={agent({ model_roles: { endpoint_id: "gw", large: "claude-opus" }, subagent_model: "medium" })} wsId="w1" />);
    await roleSelect("Large (opus)");
    expect(screen.queryAllByTestId("role-warning")).toHaveLength(0);
    expect(screen.queryByText(/is not allowed for your organization/)).toBeNull();
  });
});

describe("Instructions at Use (D-406)", () => {
  it("shows Name, Instructions and Model and one collapsed line in place of the Expert group; Show opens it in place", async () => {
    mockFetch(base());
    wrapWith(<InstructionsTab agent={agent()} wsId="w1" />, { moreControls: false });
    expect(screen.getByLabelText("Name")).toBeInTheDocument();
    expect(screen.getByLabelText("Instructions")).toBeInTheDocument();
    expect(screen.getByLabelText("Model")).toBeInTheDocument();
    // Expert strings never render at Use: the harness, the roles, Subagents use and Effort.
    expect(screen.queryByLabelText("Harness")).toBeNull();
    expect(screen.queryByLabelText("Effort")).toBeNull();
    expect(screen.queryByLabelText("Large (opus)")).toBeNull();
    expect(screen.getByText("Model roles, effort and agent program")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show" }));
    expect(await screen.findByLabelText("Effort")).toBeInTheDocument();
  });
});
