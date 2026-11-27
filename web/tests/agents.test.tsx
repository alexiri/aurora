import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ConnectTab } from "@/components/agents/connect-tab";
import { CreateAgentDialog } from "@/components/agents/create-agent-dialog";
import { WorkspaceNav } from "@/components/app-shell";
import { UndoToastHost } from "@/components/ui/undo-toast";
import { wrap } from "./support/render";
import { json } from "./support/api";

const push = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: vi.fn(), push }), useParams: () => ({ org: "acme", ws: "ops" }) }));

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
  updated_at: "2026-01-01T00:00:00Z", subagent_model: "inherit", narration_cards: false, needs_update: false,
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
  await screen.findByLabelText("Where it runs");
  if (kind === "byo") {
    // BYO sits under "More options" (P5-REFINE-12).
    fireEvent.click(screen.getByText(/More options: bring your own agent/));
    fireEvent.change(screen.getByLabelText("Where it runs"), { target: { value: "byo" } });
  }
  fireEvent.click(screen.getByRole("button", { name: "Create agent" }));
}

const postedBody = async (fn: ReturnType<typeof mockFetch>) => {
  await waitFor(() => expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST")).toBe(true));
  const post = fn.mock.calls.map(([r]) => r).find((r): r is Request => r instanceof Request && r.method === "POST")!;
  return (await post.clone().json()) as Record<string, unknown>;
};

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
  it("offers no hosted option when the feature is off and says so", async () => {
    mockFetch({ "GET /v1/instance": () => json(200, instance(false)) });
    wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    expect(await screen.findByText(/This instance has no hosted agents/)).toBeInTheDocument();
    expect(screen.queryByLabelText("Where it runs")).not.toBeInTheDocument();
  });
  it("creates a hosted agent by default (Runs on Aurora), with BYO only under More options", async () => {
    const fn = mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "POST /v1/workspaces/w1/agents": () => json(201, { agent: agent("hosted"), token: null, container_config: null }),
    });
    wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "Bot" } });
    const where = await screen.findByLabelText("Where it runs");
    expect((where as HTMLSelectElement).value).toBe("hosted");
    expect(screen.getByRole("option", { name: "Runs on Aurora (recommended)" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /my own computer/ })).not.toBeInTheDocument();
    expect(screen.getByText("Aurora hosts the agent and its computer for you.")).toBeInTheDocument();
    expect(screen.queryByLabelText("Brief")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Create agent" }));
    expect((await postedBody(fn)).kind).toBe("hosted");
  });
  it("starts from General assistant: the template fills Instructions, the handle follows the name", async () => {
    const fn = mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "POST /v1/workspaces/w1/agents": () => json(201, { agent: agent("hosted"), token: null, container_config: null }),
    });
    wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    expect(await screen.findByRole("radio", { name: "General assistant" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual([
      "General assistant", "Research", "Ops helper", "Calendar secretary", "Writer", "A blank agent",
    ]);
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Rex The Helper" } });
    expect((screen.getByLabelText("Handle (optional)") as HTMLInputElement).value).toBe("rex-the-helper");
    expect((screen.getByLabelText("Instructions (optional)") as HTMLTextAreaElement).value).toContain("You are Rex The Helper");
    fireEvent.click(screen.getByRole("button", { name: "Create agent" }));
    const body = await postedBody(fn);
    expect(body.handle).toBe("rex-the-helper");
    expect(String(body.brief)).toContain("Never change anything outside this chat without asking first.");
  });
  it("a template fills Name only while it is empty, and a blank agent leaves Instructions empty", async () => {
    mockFetch({ "GET /v1/instance": () => json(200, instance(true)) });
    wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    fireEvent.click(await screen.findByRole("radio", { name: "Research" }));
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Researcher");
    fireEvent.click(screen.getByRole("radio", { name: "Writer" }));
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Researcher");
    expect((screen.getByLabelText("Instructions (optional)") as HTMLTextAreaElement).value).toContain("You are Researcher, a writing assistant");
    fireEvent.click(screen.getByRole("radio", { name: "A blank agent" }));
    expect((screen.getByLabelText("Instructions (optional)") as HTMLTextAreaElement).value).toBe("");
  });
  it("a hosted agent opens in its chat with the template's first message as a draft, and a toast", async () => {
    push.mockClear();
    mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "POST /v1/workspaces/w1/agents": () => json(201, { agent: agent("hosted"), token: null, container_config: null }),
    });
    wrap(
      <>
        <CreateAgentDialog wsId="w1" />
        <UndoToastHost />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "Bot" } });
    fireEvent.click(screen.getByRole("button", { name: "Create agent" }));
    await waitFor(() => expect(push).toHaveBeenCalledTimes(1));
    const href = String(push.mock.calls[0][0]);
    expect(href.startsWith("/o/acme/w/ops/agents/bot?")).toBe(true);
    expect(new URL(href, "http://x").searchParams.get("draft")).toBe("Hi Bot, what can you do for me?");
    expect(await screen.findByText("Bot is ready")).toBeInTheDocument();
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
