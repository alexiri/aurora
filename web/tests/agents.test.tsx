import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ConnectTab } from "@/components/agents/connect-tab";
import { CreateAgentDialog } from "@/components/agents/create-agent-dialog";
import { WorkspaceNav } from "@/components/app-shell";

vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: vi.fn() }) }));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

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

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const agent = (kind: "byo" | "hosted") => ({
  id: "a1", org_id: "o1", workspace_id: "w1", handle: "bot", name: "Bot", kind,
  is_default: false, harness: null, model_provider: null, model_endpoint_id: null, model_id: null,
  effort: null, proactive_messaging: "ask", brief: "", created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
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
  wrap(<CreateAgentDialog wsId="w1" />);
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
    wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    await screen.findByLabelText("Kind");
    expect(screen.queryByRole("option", { name: "Hosted" })).not.toBeInTheDocument();
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
    fireEvent.click(screen.getByRole("button", { name: "Yes, regenerate" }));
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

describe("workspace nav", () => {
  const org = (role: string) => ({ id: "o1", slug: "acme", name: "Acme", my_role: role });
  function navWith(role: string) {
    mockFetch({
      "GET /v1/resolve": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" } }),
      "GET /v1/orgs/o1": () => json(200, org(role)),
    });
    wrap(<WorkspaceNav base="/o/acme/w/main" orgSlug="acme" wsId={undefined} pathname="/o/acme/w/main" />);
  }
  it("links Members for admins", async () => {
    navWith("admin");
    expect(await screen.findByRole("link", { name: "Members" })).toHaveAttribute("href", "/o/acme/members");
  });
  it("hides Members for plain members", async () => {
    navWith("member");
    await screen.findByRole("link", { name: "Agents" });
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("link", { name: "Members" })).not.toBeInTheDocument();
    expect(screen.queryByText(/Members/)).not.toBeInTheDocument();
  });
});
