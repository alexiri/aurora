import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { chooseService } from "./support/llm";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/credentials",
  useSearchParams: () => new URLSearchParams(window.location.search),
}));
/** The page URL the tab hook reads (`?tab=`) and replaces (History API, like the chat's `?c=`). */
const setUrl = (search: string) => window.history.replaceState(null, "", `/o/acme/w/ops/credentials${search ? `?${search}` : ""}`);
vi.mock("@/components/apps/apps-tab", () => ({ AppsTab: () => <div>apps-tab</div> }));
vi.mock("@/components/apps/oauth-clients-panel", () => ({ OAuthClientsPanel: () => <div>oauth-tab</div> }));

import { AccessTab } from "@/components/agents/access-tab";
import { ModelReadinessSection } from "@/components/agents/model-readiness";
import { AccessDialog, GiveAccessStep, type GrantSubject } from "@/components/credentials/give-access";
import { LlmKeyForm } from "@/components/credentials/llm-form";
import { OriginView, originDetailText } from "@/components/credentials/origin-view";
import { errorCopy } from "@/lib/chat/transcript";
import { resolveTab } from "@/lib/use-tab-param";

type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
interface Call { key: string; body: unknown }
let calls: Call[];
let routes: Record<string, Handler>;

beforeEach(() => {
  calls = [];
  routes = {};
  setUrl("");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const body = text ? JSON.parse(text) : undefined;
      const key = `${req.method} ${url.pathname}`;
      calls.push({ key, body });
      const h = routes[key];
      return h ? h(req, body) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});

const sec = (over: Record<string, unknown> = {}) => ({
  id: "s1", scope: "workspace", kind: "generic", name: "gh", status: "active", host_pattern: "api.github.com", path_pattern: null,
  injection: { kind: "bearer" }, value_source: "value", value_version: 1, llm: null, origin: null, ...over,
});
const ws = { id: "w1", org_id: "o1", name: "Ops", my_role: "owner" };
const agent = (id: string, name: string, kind = "hosted") => ({ id, handle: name.toLowerCase(), name, kind, is_default: false, frozen: null });

// ---------------------------------------------------------------------------------------------------------------------
describe("tab values", () => {
  it("resolves a value or falls back", () => {
    expect(resolveTab("llm", ["secrets", "llm"], "secrets")).toBe("llm");
    expect(resolveTab("nope", ["secrets", "llm"], "secrets")).toBe("secrets");
    expect(resolveTab(null, ["secrets", "llm"], "secrets")).toBe("secrets");
  });
});

// ---------------------------------------------------------------------------------------------------------------------
describe("error links", () => {
  it("no_model_key opens LLM keys and offers Agent access to owners; no_default_model opens Instructions", () => {
    const a = errorCopy("no_model_key");
    expect(a.link).toMatchObject({ label: "Open AI keys", path: "credentials", tab: "llm" });
    expect(a.extra).toMatchObject({ label: "Agent access", path: "access", ownerOnly: true });
    const b = errorCopy("no_default_model");
    expect(b.message).toBe("This agent has an LLM key, but Aurora could not pick a model for it.");
    expect(b.link).toMatchObject({ label: "Choose a model", path: "instructions" });
    expect(b.guideStep).toBeUndefined(); // (FIX-170) the direct link is the fix
    expect(a.guideStep).toBe("key");
  });
});

// ---------------------------------------------------------------------------------------------------------------------
describe("origin view", () => {
  const base = { value_version: 1 };
  it("shows 'Verified (endpoint)' for the endpoint method", () => {
    render(<OriginView origin={{ ...base, status: "verified", method: "endpoint" }} />);
    expect(screen.getByText("Verified (endpoint)")).toBeTruthy();
    expect(screen.queryByText(/Verified account/)).toBeNull();
  });
  it("shows the method and the ids for a team key", () => {
    render(<OriginView origin={{ ...base, status: "verified", method: "litellm_team", team_id: "team-7", organization_id: "org-1" }} />);
    expect(screen.getByText("Verified account:")).toBeTruthy();
    expect(screen.getByText("LiteLLM team")).toBeTruthy();
    expect(screen.getByText("team-7")).toBeTruthy();
    expect(screen.getByText("org-1")).toBeTruthy();
  });
  it("shows a teamless key as verified with no team", () => {
    render(<OriginView origin={{ ...base, status: "verified", method: "litellm_team", team_id: null }} />);
    expect(screen.getByText("none (the key reports no team)")).toBeTruthy();
  });
  it("shows the detail of a failure in words", () => {
    const { unmount } = render(<OriginView origin={{ ...base, status: "unverifiable", method: "litellm_team", detail: "key_info_http_401" }} />);
    expect(screen.getByText("The account could not be verified")).toBeTruthy();
    expect(screen.getByText("LiteLLM refused the key-info call (401)")).toBeTruthy();
    unmount();
    render(<OriginView origin={{ ...base, status: "mismatch", method: "litellm_team", detail: "key_reports_no_team" }} />);
    expect(screen.getByText("This key belongs to no LiteLLM team")).toBeTruthy();
    expect(originDetailText("endpoint_unreachable")).toBe("The endpoint could not be reached");
    expect(originDetailText(null)).toBeNull();
  });
  it("is not labelled 'Verified account: verified' any more", () => {
    render(<OriginView origin={{ ...base, status: "verified", method: "provider_account", organization_id: "org-1" }} />);
    expect(screen.queryByText("verified")).toBeNull();
  });
});

// ---------------------------------------------------------------------------------------------------------------------
const ROLES_OK = {
  roles: {
    large: { model_id: null, source: "none", warning: null },
    medium: { model_id: null, source: "none", warning: null },
    small: { model_id: null, source: "none", warning: null },
  },
  roles_dropped: [],
  subagent_model: "inherit",
  subagent_warning: null,
};

describe("model readiness on the Access tab", () => {
  const readiness = (keys: unknown[], dflt: unknown, over: Record<string, unknown> = {}) =>
    json(200, { harness: "claude-code", keys, default: dflt, override_dropped: false, ...over });
  const key = (over: Record<string, unknown>) => ({
    secret_id: "k" + Math.random(), name: "Team key", scope: "workspace", endpoint_id: "aigw", provider: "litellm", usable: true, reason: null, ...over,
  });
  // (P5-POLISH-6, D-385) The Access page no longer shows this block (it shows one "Can't answer" line); the section is kept
  // for the endpoint's page, so these tests render it on its own.
  const tab = () => wrap(<ModelReadinessSection agentId="a1" base="/o/acme/w/ops" />);
  beforeEach(() => {
    routes["GET /v1/workspaces/w1/secrets"] = () => json(200, { items: [] });
    routes["GET /v1/orgs/o1/secrets"] = () => json(200, { items: [] });
    routes["GET /v1/workspaces/w1/accounts"] = () => json(200, { items: [], next_cursor: null });
  });

  it.each([
    ["not_active", /not active/, "Accept it or replace the value."],
    ["endpoint_unknown", /no longer configured/, "Ask your operator, or add a key on a configured endpoint."],
    ["provider_disabled", /provider is disabled/, "Ask your operator to enable it."],
    ["scope_not_permitted", /does not permit this key scope/, "An organization admin can change this."],
    ["origin_unverified", /not verified/, "Re-verify the key."],
    ["origin_not_compliant", /not an allowed one/, "Use a key from an allowed account."],
    ["no_shared_dialect", /no dialect/, "Ask your operator to add a dialect, or use another harness."],
  ])("explains %s in words with its fix", async (reason, text, fix) => {
    routes["GET /v1/agents/a1/model-readiness"] = () => readiness([key({ usable: false, reason })], null);
    tab();
    const list = await screen.findByRole("list", { name: "Granted LLM keys" });
    expect(within(list).getByText("Not usable")).toBeTruthy();
    expect(within(list).getByText(text)).toBeTruthy();
    expect(list.textContent).toContain(fix);
  });

  it("links the fix to the right page", async () => {
    routes["GET /v1/agents/a1/model-readiness"] = () =>
      readiness([key({ usable: false, reason: "origin_unverified" }), key({ name: "Org key", scope: "organization", usable: false, reason: "scope_not_permitted" })], null);
    tab();
    expect((await screen.findByRole("link", { name: "Re-verify the key." })).getAttribute("href")).toBe("/o/acme/w/ops/apps?tab=accounts&kind=ai-key");
    expect(screen.getByRole("link", { name: "An organization admin can change this." }).getAttribute("href")).toBe("/o/acme/ai-models");
  });

  it("shows Ready and the model the next turn uses", async () => {
    routes["GET /v1/agents/a1/model-readiness"] = () =>
      readiness([key({})], { endpoint_id: "aigw", provider: "litellm", model_id: "gpt-4o", source: "endpoint_default", ...ROLES_OK });
    tab();
    expect(await screen.findByText("Ready")).toBeTruthy();
    const next = screen.getByTestId("next-model");
    expect(next.textContent).toContain("gpt-4o");
    expect(next.textContent).toContain("the endpoint's default model");
  });

  // (phase 5, P5-MODELS-3, C-878) Resolved roles with their source, the warnings, the dropped roles and the subagent default.
  it("lists the resolved roles with their source, an unset role on LiteLLM, a dropped role and a role not allowed for the org", async () => {
    routes["GET /v1/agents/a1/model-readiness"] = () =>
      readiness([key({})], {
        endpoint_id: "aigw", provider: "litellm", model_id: "gpt-4o", source: "override",
        roles: {
          large: { model_id: "claude-opus", source: "agent", warning: "role_not_allowed" },
          medium: { model_id: "glm-4.6", source: "endpoint", warning: null },
          small: { model_id: null, source: "none", warning: "role_unset" },
        },
        roles_dropped: ["small"], subagent_model: "small", subagent_warning: "subagent_role_unset",
      });
    tab();
    const list = await screen.findByRole("list", { name: "Model roles" });
    const rows = within(list).getAllByRole("listitem").map((r) => r.textContent);
    expect(rows[0]).toContain("Strongest modelclaude-opusset on this agent");
    expect(rows[1]).toContain("Everyday modelglm-4.6from the endpoint");
    expect(rows[2]).toContain("Fastest modelnonenot set");
    const notes = screen.getAllByTestId("role-warning").map((n) => n.textContent ?? "");
    expect(notes.some((t) => t.includes("Strongest model (claude-opus) is not allowed for your organization"))).toBe(true);
    expect(notes.some((t) => t.includes("Fastest model is not set. On this endpoint, subagents and background requests may ask for a model it does not serve."))).toBe(true);
    expect(notes.some((t) => t.includes("The Fastest model set for this agent is no longer usable"))).toBe(true);
    expect(notes.some((t) => t.includes("Subagents use Fastest model, which is not set"))).toBe(true);
    expect(screen.getByTestId("subagent-model").textContent).toBe("Subagents use Fastest model.");
    expect(screen.getAllByRole("link", { name: "Change the roles on Instructions" })[0]!.getAttribute("href")).toContain("tab=instructions");
  });

  it("says 'No model chosen' with the Instructions link when default is null", async () => {
    routes["GET /v1/agents/a1/model-readiness"] = () => readiness([key({})], null);
    tab();
    expect(await screen.findByText(/No model chosen/)).toBeTruthy();
    expect(screen.getByRole("link", { name: "Choose one on Instructions" }).getAttribute("href")).toBe("/o/acme/w/ops/credentials?tab=instructions#a-model"); // relative to the (mocked) current path
    expect(screen.queryByTestId("next-model")).toBeNull();
  });

  it("warns when the agent's model override would be dropped, and when no key is granted", async () => {
    routes["GET /v1/agents/a1/model-readiness"] = () => readiness([], null, { override_dropped: true });
    tab();
    expect(await screen.findByText(/No LLM key is granted/)).toBeTruthy();
    expect(screen.getByText(/will ignore it/)).toBeTruthy();
    expect(screen.queryByText(/No model chosen/)).toBeNull();
  });

  it("the Access page shows no readiness block, and makes no readiness request for non-owners or BYO agents", async () => {
    routes["GET /v1/agents/a1/model-readiness"] = () => readiness([key({ usable: false, reason: "not_active" })], null);
    wrap(<AccessTab agentId="a1" agentName="Daneel" wsId="w1" orgId="o1" canOwn={false} hosted base="/o/acme/w/ops" />);
    await screen.findByText("Nothing is given yet");
    cleanup();
    wrap(<AccessTab agentId="a1" agentName="Daneel" wsId="w1" orgId="o1" canOwn hosted={false} base="/o/acme/w/ops" />);
    await screen.findByText("Nothing is given yet");
    expect(calls.some((c) => c.key.includes("model-readiness"))).toBe(false);
    cleanup();
    wrap(<AccessTab agentId="a1" agentName="Daneel" wsId="w1" orgId="o1" canOwn hosted base="/o/acme/w/ops" />);
    expect(await screen.findByTestId("cant-answer")).toHaveTextContent("Can't answer");
    expect(screen.queryByRole("list", { name: "Granted LLM keys" })).toBeNull();
  });
});

// ---------------------------------------------------------------------------------------------------------------------
describe("Give agents access step", () => {
  const wsSubject = (target: GrantSubject["target"], extra: Partial<GrantSubject> = {}): GrantSubject => ({
    target, scope: { kind: "workspace", id: "w1" }, orgId: "o1", ...extra,
  });
  const agents = [agent("a1", "Daneel"), agent("a2", "Giskard"), agent("a3", "Olivaw")];
  beforeEach(() => {
    routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: agents, next_cursor: null });
    routes["GET /v1/secrets/s1/grants"] = () => json(200, { items: [] });
    routes["GET /v1/connections/c1/grants"] = () => json(200, { items: [] });
  });
  const step = (subject: GrantSubject, onDone = vi.fn()) => {
    wrap(<GiveAccessStep subject={subject} savedLabel="gh" onDone={onDone} />);
    return onDone;
  };
  const check = async (name: string) => fireEvent.click(await screen.findByRole("checkbox", { name: `Give ${name} access` }));
  // (C-925) The button names the result: "Give Ada access" with one agent ticked.
  const give = () => fireEvent.click(screen.getByRole("button", { name: /^Give .*access$/ }));

  it("lists the workspace's agents with nothing preselected and Give access disabled", async () => {
    step(wsSubject({ kind: "secret", id: "s1" }));
    const boxes = await screen.findAllByRole("checkbox");
    expect(boxes).toHaveLength(3);
    expect(boxes.every((b) => !(b as HTMLInputElement).checked)).toBe(true);
    expect((screen.getByRole("button", { name: /^Give .*access$/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByRole("button", { name: "Skip" })).toBeTruthy();
  });

  it("Skip ends the step without any grant request", async () => {
    const done = step(wsSubject({ kind: "secret", id: "s1" }));
    await screen.findAllByRole("checkbox");
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    expect(done).toHaveBeenCalled();
    expect(calls.some((c) => c.key.startsWith("POST"))).toBe(false);
  });

  it("gives a secret to the selected agents (C-503)", async () => {
    routes["POST /v1/secrets/s1/grants"] = (_r, body) =>
      json(200, { items: (body as { agent_ids: string[] }).agent_ids.map((id) => ({ agent_id: id, grant: {}, status: "created" })) });
    step(wsSubject({ kind: "secret", id: "s1" }));
    await check("Daneel");
    await check("Olivaw");
    give();
    expect(await screen.findByText("Access given to 2 agents.")).toBeTruthy();
    expect(calls.find((c) => c.key === "POST /v1/secrets/s1/grants")?.body).toEqual({ agent_ids: ["a1", "a3"] });
    expect(screen.getByRole("button", { name: "Done" })).toBeTruthy();
  });

  it("shows agents that already have access as such, and reports partial-existing", async () => {
    routes["GET /v1/secrets/s1/grants"] = () =>
      json(200, { items: [{ agent_id: "a2", agent_name: "Giskard", workspace_id: "w1", workspace_name: "Ops", secret_id: "s1", id: "g", created_at: "" }] });
    routes["POST /v1/secrets/s1/grants"] = () =>
      json(200, { items: [{ agent_id: "a1", grant: {}, status: "created" }, { agent_id: "a3", grant: {}, status: "existing" }] });
    step(wsSubject({ kind: "secret", id: "s1" }));
    await screen.findByText("Has access");
    const has = screen.getByRole("checkbox", { name: "Give Giskard access" }) as HTMLInputElement;
    expect(has.checked).toBe(true);
    expect(has.disabled).toBe(true);
    await check("Daneel");
    await check("Olivaw");
    give();
    expect(await screen.findByText("Access given to 1 agent (1 already had it).")).toBeTruthy();
    expect(calls.find((c) => c.key === "POST /v1/secrets/s1/grants")?.body).toEqual({ agent_ids: ["a1", "a3"] });
  });

  it("names the failing agent and says nothing was changed", async () => {
    routes["POST /v1/secrets/s1/grants"] = () =>
      json(403, { error: { code: "forbidden", message: "You are not allowed to do that.", details: { agent_id: "a2" } } });
    step(wsSubject({ kind: "secret", id: "s1" }));
    await check("Daneel");
    await check("Giskard");
    give();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Could not give access to Giskard");
    expect(alert.textContent).toContain("No agent was changed");
  });

  it("grants a connection full access (C-504)", async () => {
    routes["GET /v1/apps/github"] = () => json(200, { app_id: "github", name: "GitHub", tools });
    routes["POST /v1/connections/c1/grants"] = () => json(200, { items: [{ agent_id: "a1", grant: {}, status: "created" }] });
    step(wsSubject({ kind: "connection", id: "c1" }, { appId: "github", appWorkspaceId: "w1" }));
    await check("Daneel");
    give();
    await screen.findByText("Access given to 1 agent.");
    expect(calls.find((c) => c.key === "POST /v1/connections/c1/grants")?.body).toEqual({ agent_ids: ["a1"], mode: "full" });
  });

  const tools = [
    { id: "issues.read", name: "Read issues", group: "read", host: "api.github.com", methods: ["GET"], path: "/issues", connect_methods: [], wildcard: false },
    { id: "issues.write", name: "Create issues", group: "write", host: "api.github.com", methods: ["POST"], path: "/issues", connect_methods: [], wildcard: false },
  ];

  it("starts a custom_only app on Custom with every tool blocked and sends the chosen grant (the Access tab's form)", async () => {
    routes["GET /v1/apps/github"] = () => json(200, { app_id: "github", name: "GitHub", tools });
    routes["POST /v1/connections/c1/grants"] = () => json(200, { items: [{ agent_id: "a1", grant: {}, status: "created" }] });
    step(wsSubject({ kind: "connection", id: "c1" }, { appId: "github", appWorkspaceId: "w1", customOnly: true }));
    await check("Daneel");
    expect(await screen.findByText(/This app only supports custom access/)).toBeTruthy();
    expect(screen.queryByRole("radiogroup", { name: "Access mode" })).toBeNull();
    expect((screen.getByRole("button", { name: /^Give .*access$/ }) as HTMLButtonElement).disabled).toBe(true); // nothing allowed yet
    fireEvent.click(within(screen.getByRole("radiogroup", { name: "Access for Read issues" })).getByLabelText("Allow"));
    fireEvent.click(within(screen.getByRole("radiogroup", { name: "Access for Create issues" })).getByLabelText("Ask"));
    fireEvent.change(screen.getByLabelText("Approval timeout (seconds)"), { target: { value: "600" } });
    give();
    await screen.findByText("Access given to 1 agent.");
    expect(calls.find((c) => c.key === "POST /v1/connections/c1/grants")?.body).toEqual({
      agent_ids: ["a1"], mode: "custom", allow_tools: ["issues.read"], ask_tools: ["issues.write"], ask_timeout_secs: 600,
    });
  });

  it("falls back to Custom when the server answers full_grant_forbidden", async () => {
    routes["GET /v1/apps/github"] = () => json(200, { app_id: "github", name: "GitHub", tools });
    routes["POST /v1/connections/c1/grants"] = (_r, body) =>
      (body as { mode: string }).mode === "full"
        ? json(422, { error: { code: "full_grant_forbidden", message: "This app allows only custom grants.", details: { agent_id: "a1" } } })
        : json(200, { items: [{ agent_id: "a1", grant: {}, status: "created" }] });
    step(wsSubject({ kind: "connection", id: "c1" }, { appId: "github", appWorkspaceId: "w1" }));
    await check("Daneel");
    give();
    const row = await screen.findByRole("radiogroup", { name: "Access for Create issues" }); // the form switched to Custom
    fireEvent.click(within(row).getByLabelText("Allow"));
    give();
    await screen.findByText("Access given to 1 agent.");
    expect(calls.filter((c) => c.key === "POST /v1/connections/c1/grants").map((c) => (c.body as { mode: string }).mode)).toEqual(["full", "custom"]);
  });

  it("groups agents by the workspaces the caller owns for an organization credential", async () => {
    routes["GET /v1/orgs/o1/workspaces"] = () =>
      json(200, {
        items: [
          { id: "w1", org_id: "o1", name: "Ops", my_role: "owner" },
          { id: "w2", org_id: "o1", name: "Research", my_role: "owner" },
          { id: "w3", org_id: "o1", name: "Finance", my_role: "member" },
        ],
      });
    routes["GET /v1/workspaces/w2/agents"] = () => json(200, { items: [agent("b1", "Hari")], next_cursor: null });
    step({ target: { kind: "secret", id: "s1" }, scope: { kind: "organization", id: "o1" }, orgId: "o1" });
    expect(await screen.findByText("Ops")).toBeTruthy();
    expect(screen.getByText("Research")).toBeTruthy();
    expect(screen.queryByText("Finance")).toBeNull();
    expect(await screen.findByRole("checkbox", { name: "Give Hari access" })).toBeTruthy();
    expect(calls.some((c) => c.key === "GET /v1/workspaces/w3/agents")).toBe(false);
  });

  it("a workspace's AI key is saved with its agents in one request (C-988), the only agent preticked", async () => {
    routes["GET /v1/llm/providers"] = () =>
      json(200, {
        providers: [{ id: "litellm", display_name: "LiteLLM", dialects: [], auth_modes: ["api_key"] }],
        endpoints: [{ id: "aigw", provider: "litellm", display_name: "AI gateway", base_url: "https://x", dialects: [], permitted: true, key_scopes: ["workspace"] }],
      });
    routes["POST /v1/workspaces/w1/secrets"] = () =>
      json(201, sec({ id: "s1", kind: "llm", name: "Team key", llm: { endpoint_id: "aigw", provider: "litellm", auth_mode: "api_key" }, origin: { status: "verified", method: "endpoint", value_version: 1 } }));
    routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [agent("a1", "Daneel")], next_cursor: null });
    wrap(<LlmKeyForm scope={{ kind: "workspace", id: "w1" }} orgId="o1" />);
    await chooseService("AI gateway");
    const box = await screen.findByRole("checkbox", { name: "Daneel can use this key" });
    expect(box).toBeChecked();
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Team key" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Add LLM key" }));
    expect(await screen.findByText("Saved Team key")).toBeTruthy();
    expect(screen.getByText("Verified (endpoint)")).toBeTruthy();
    expect(screen.queryByLabelText("API key")).toBeNull();
    const posts = calls.filter((c) => c.key.startsWith("POST"));
    expect(posts.map((c) => c.key)).toEqual(["POST /v1/workspaces/w1/secrets"]);
    expect((posts[0].body as { grant: unknown }).grant).toEqual({ agent_ids: ["a1"] });
  });

  it("with several agents nothing is preticked and no grant is sent; an organization's key keeps the two steps", async () => {
    routes["GET /v1/llm/providers"] = () =>
      json(200, {
        providers: [{ id: "litellm", display_name: "LiteLLM", dialects: [], auth_modes: ["api_key"] }],
        endpoints: [{ id: "aigw", provider: "litellm", display_name: "AI gateway", base_url: "https://x", dialects: [], permitted: true, key_scopes: ["workspace", "organization"] }],
      });
    routes["POST /v1/workspaces/w1/secrets"] = () => json(201, sec({ id: "s1", kind: "llm", name: "Team key", llm: { endpoint_id: "aigw", provider: "litellm", auth_mode: "api_key" }, origin: null }));
    routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [agent("a1", "Daneel"), agent("a2", "Giskard")], next_cursor: null });
    wrap(<LlmKeyForm scope={{ kind: "workspace", id: "w1" }} orgId="o1" />);
    await chooseService("AI gateway");
    expect(await screen.findByRole("checkbox", { name: "Daneel can use this key" })).not.toBeChecked();
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Team key" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Add LLM key" }));
    await screen.findByText("Saved Team key");
    expect(calls.find((c) => c.key === "POST /v1/workspaces/w1/secrets")?.body).not.toHaveProperty("grant");
    cleanup();
    routes["POST /v1/orgs/o1/secrets"] = () => json(201, sec({ id: "s2", scope: "organization", kind: "llm", name: "Org key", llm: { endpoint_id: "aigw", provider: "litellm", auth_mode: "api_key" }, origin: null }));
    wrap(<LlmKeyForm scope={{ kind: "organization", id: "o1" }} orgId="o1" />);
    await chooseService("AI gateway");
    expect(screen.queryByRole("checkbox", { name: /can use this key/ })).toBeNull();
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Org key" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-2" } });
    fireEvent.click(screen.getByRole("button", { name: "Add LLM key" }));
    expect(await screen.findByRole("region", { name: "Give agents access" })).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------------------------------------------------
describe("row Access dialog", () => {
  const subject: GrantSubject = { target: { kind: "secret", id: "s1" }, scope: { kind: "workspace", id: "w1" }, orgId: "o1" };
  const gv = (id: string, name: string) => ({ agent_id: id, agent_name: name, workspace_id: "w1", workspace_name: "Ops", secret_id: "s1", id: "g" + id, created_at: "" });
  beforeEach(() => {
    routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [agent("a1", "Daneel"), agent("a2", "Giskard")], next_cursor: null });
  });
  const open = () => wrap(<AccessDialog subject={subject} title="gh" open onOpenChange={() => undefined} />);

  it("lists the agents that have the credential", async () => {
    routes["GET /v1/secrets/s1/grants"] = () => json(200, { items: [gv("a1", "Daneel")] });
    open();
    const dlg = await screen.findByRole("dialog", { name: "Access to gh" });
    expect(await within(dlg).findByRole("button", { name: "Remove access for Daneel" })).toBeTruthy();
    expect(within(dlg).getByText("Has access")).toBeTruthy();
  });

  it("removes through the agent-side DELETE", async () => {
    routes["GET /v1/secrets/s1/grants"] = () => json(200, { items: [gv("a1", "Daneel")] });
    routes["DELETE /v1/agents/a1/grants/secrets/s1"] = () => new Response(null, { status: 204 });
    open();
    fireEvent.click(await screen.findByRole("button", { name: "Remove access for Daneel" }));
    await waitFor(() => expect(calls.some((c) => c.key === "DELETE /v1/agents/a1/grants/secrets/s1")).toBe(true));
  });

  it("names the agent when a removal fails", async () => {
    routes["GET /v1/secrets/s1/grants"] = () => json(200, { items: [gv("a1", "Daneel")] });
    routes["DELETE /v1/agents/a1/grants/secrets/s1"] = () => json(403, { error: { code: "forbidden", message: "Nope." } });
    open();
    fireEvent.click(await screen.findByRole("button", { name: "Remove access for Daneel" }));
    expect((await screen.findByText(/Could not remove access for Daneel/)).textContent).toContain("Daneel");
  });

  it("adds agents with the same picker", async () => {
    routes["GET /v1/secrets/s1/grants"] = () => json(200, { items: [gv("a1", "Daneel")] });
    routes["POST /v1/secrets/s1/grants"] = () => json(200, { items: [{ agent_id: "a2", grant: {}, status: "created" }] });
    open();
    await screen.findByRole("button", { name: "Remove access for Daneel" });
    // (C-925) Giskard is the only agent that can still be chosen: ticked from the start, and the button names it.
    expect(await screen.findByRole("checkbox", { name: "Give Giskard access" })).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Give Giskard access" }));
    await screen.findByText("Access given to 1 agent.");
    expect(calls.find((c) => c.key === "POST /v1/secrets/s1/grants")?.body).toEqual({ agent_ids: ["a2"] });
    expect(screen.getByRole("button", { name: "Done" })).toBeTruthy();
  });
});

void ws;
