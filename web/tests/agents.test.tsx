import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ConnectTab } from "@/components/agents/connect-tab";
import { CreateAgentDialog } from "@/components/agents/create-agent-dialog";
import { WorkspaceNav } from "@/components/app-shell";
import { wrap } from "./support/render";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: vi.fn() }) }));

function mockFetch(routes: Record<string, () => Response>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : null;
    const url = new URL(req ? req.url : String(input), "http://localhost");
    const h = routes[`${req?.method ?? "GET"} ${url.pathname}`];
    return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

const agent = (kind: "byo" | "hosted") => ({
  id: "a1", org_id: "o1", workspace_id: "w1", handle: "bot", name: "Bot", kind,
  is_default: false, harness: null, model_provider: null, model_endpoint_id: null, model_id: null,
  effort: null, proactive_messaging: "ask", brief: "", effective_frozen: false, created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z", subagent_model: "inherit", narration_cards: false,
});
const config = {
  proxy_url: "https://aug_SECRET@gw:1", env: { A: "b" }, ca_pem: "", ca_path: "/ca.pem", proxy_ca_path: "/ca.pem", proxy_tls: null, llm_env: {}, stubs: [],
};
const instance = (hosted: boolean) => ({ features: { hosted, channels: false } });

async function createFlow(kind: "byo" | "hosted") {
  mockFetch({
    "GET /v1/instance": () => json(200, instance(true)),
    "POST /v1/workspaces/w1/agents": () =>
      json(201, {
        agent: agent(kind),
        token: kind === "byo" ? "aug_SECRET" : null,
        container_config: kind === "byo" ? config : null,
      }),
  });
  // The kind select is Expert (D-406): these flows pick a kind, so the switch is on.
  wrap(<CreateAgentDialog wsId="w1" />, { moreControls: true });
  fireEvent.click(screen.getByRole("button", { name: "New agent" }));
  fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "Bot" } });
  if (kind === "hosted") {
    await waitFor(() => expect(screen.getByRole("option", { name: "Hosted" })).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText("Kind"), { target: { value: "hosted" } });
  }
  fireEvent.click(screen.getByRole("button", { name: "Create agent" }));
}

describe("create agent", () => {
  it("reveals the BYO token once and clears it on close", async () => {
    await createFlow("byo");
    expect(await screen.findByTestId("secret-key")).toHaveTextContent("aug_SECRET");
    expect(screen.getByTestId("revealed-config")).toBeInTheDocument();
    expect(screen.getByTestId("proxy-tls-note")).toHaveTextContent("TLS-only");
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    fireEvent.click(screen.getByRole("button", { name: "Close anyway" }));
    await waitFor(() => expect(screen.queryByTestId("secret-key")).not.toBeInTheDocument());
    expect(document.body.textContent).not.toContain("aug_SECRET");
  });
  it("does not reveal anything for hosted agents", async () => {
    await createFlow("hosted");
    await waitFor(() => expect(screen.queryByRole("button", { name: "Create agent" })).not.toBeInTheDocument());
    expect(screen.queryByTestId("secret-key")).not.toBeInTheDocument();
  });
  it("hides the hosted option when the feature is off", async () => {
    mockFetch({ "GET /v1/instance": () => json(200, instance(false)) });
    wrap(<CreateAgentDialog wsId="w1" />, { moreControls: true });
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    await screen.findByLabelText("Kind");
    expect(screen.queryByRole("option", { name: "Hosted" })).not.toBeInTheDocument();
  });
  it("has no Kind select at Use and creates a hosted agent (D-406)", async () => {
    const fn = mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "POST /v1/workspaces/w1/agents": () => json(201, { agent: agent("hosted"), token: null, container_config: null }),
    });
    wrap(<CreateAgentDialog wsId="w1" />, { moreControls: false });
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "Bot" } });
    expect(screen.queryByLabelText("Kind")).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("button", { name: "Create agent" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Create agent" }));
    await waitFor(() => expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST")).toBe(true));
    const post = fn.mock.calls.map(([r]) => r).find((r): r is Request => r instanceof Request && r.method === "POST")!;
    expect((await post.clone().json()).kind).toBe("hosted");
  });
});

describe("regenerate token", () => {
  it("asks for confirmation before calling the API", async () => {
    const fn = mockFetch({
      "GET /v1/agents/a1/container-config": () => json(200, { ...config, proxy_url: "http://<AGENT_TOKEN>@gw:1" }),
      "GET /v1/gateway/ca.pem": () => new Response("pem", { status: 200 }),
      "POST /v1/agents/a1/token": () => json(200, { token: "aug_NEW", container_config: config }),
    });
    wrap(<ConnectTab agent={agent("byo")} />);
    await screen.findByTestId("container-config");
    fireEvent.click(screen.getByRole("button", { name: "Regenerate token" }));
    const posted = () => fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST");
    expect(posted()).toBe(false);
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Regenerate token" }));
    expect(await screen.findByTestId("secret-key")).toHaveTextContent("aug_NEW");
    expect(posted()).toBe(true);
  });
  it("shows the operator hint when the CA is unavailable", async () => {
    mockFetch({
      "GET /v1/agents/a1/container-config": () => json(200, config),
      "GET /v1/gateway/ca.pem": () =>
        json(503, { error: { code: "gateway_ca_unavailable", message: "x" } }),
    });
    wrap(<ConnectTab agent={agent("byo")} />);
    expect(await screen.findByText(/Ask your operator/)).toBeInTheDocument();
  });
});

describe("workspace nav (C-961)", () => {
  // The nav lists by role and altitude: "Show more controls" off, as a person starts (D-406).
  function navWith(role: string) {
    mockFetch({
      "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: role }),
    });
    wrap(<WorkspaceNav base="/o/acme/w/main" wsId="w1" pathname="/o/acme/w/main" />, { moreControls: false });
  }
  const names = () => screen.getAllByRole("link").map((a) => a.textContent);
  it("lists Approvals, People and Settings for an owner, with the new names and no Members item (FIX-027)", async () => {
    navWith("owner");
    await screen.findByRole("link", { name: "People" });
    expect(names()).toEqual(["Overview", "Agents", "Skills", "Apps & accounts", "Internet access", "Approvals", "Activity", "People", "Settings"]);
    expect(screen.queryByRole("link", { name: "Members" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /Credentials|Policy/ })).not.toBeInTheDocument();
  });
  it("lists only Overview and Agents for a plain member at Use (D-406)", async () => {
    navWith("member");
    await screen.findByRole("link", { name: "Agents" });
    await new Promise((r) => setTimeout(r, 50));
    expect(names()).toEqual(["Overview", "Agents"]);
  });
  it("lists Skills, Apps, Internet access and Activity for a member with Show more controls on, but never People or Settings", async () => {
    mockFetch({ "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: "member" }) });
    wrap(<WorkspaceNav base="/o/acme/w/main" wsId="w1" pathname="/o/acme/w/main" />, { moreControls: true });
    await screen.findByRole("link", { name: "Activity" });
    expect(names()).toEqual(["Overview", "Agents", "Skills", "Apps & accounts", "Internet access", "Activity"]);
  });
});
