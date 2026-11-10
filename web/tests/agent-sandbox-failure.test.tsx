import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops", handle: "scout" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/agents/scout",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("@/components/agents/chat-tab", () => ({ ChatTab: () => <div data-testid="chat-tab" /> }));
vi.mock("@/components/agents/instructions-tab", () => ({ InstructionsTab: () => <div data-testid="instructions-tab" /> }));
vi.mock("@/components/agents/delete-zone", () => ({ DeleteZone: () => null }));

import { AgentPage } from "@/components/agents/agent-page";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req?: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  // `openapi-fetch` passes a Request; `chatApi` (sandbox/restart) passes a string + init.
  const fn = vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
    const req = input instanceof Request ? input : null;
    const url = new URL(req ? req.url : String(input), "http://localhost");
    const method = req?.method ?? init?.method ?? "GET";
    const h = routes[`${method} ${url.pathname}`];
    return h ? h(req ?? undefined) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const agent = {
  id: "a1", handle: "scout", name: "Scout", kind: "hosted", is_default: false, avatar_url: null, brief: null,
  frozen: null, updated_at: "2026-01-01T00:00:00Z",
};

function routes(sandbox: Record<string, unknown>) {
  return {
    "GET /v1/resolve": () => json(200, { org: { id: "o1" }, workspace: { id: "w1" } }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [agent], next_cursor: null }),
    "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", my_role: "owner", timezone: "Europe/Zurich" }),
    "GET /v1/orgs/o1": () => json(200, { id: "o1", my_role: "owner", settings: {} }),
    "GET /v1/agents/a1/sandbox": () =>
      json(200, {
        agent_id: "a1", state: "stopped", boot_id: null, runner_id: null, image: null, started_at: null,
        keep_awake_reasons: [], failure_code: null, failure_message: null, start_attempt: null,
        processes: [], ports: [], restart_to_update: false, ...sandbox,
      }),
  };
}

function renderPage(routes: Record<string, Handler>) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  mockFetch(routes);
  return render(
    <QueryClientProvider client={qc}>
      <AgentPage />
    </QueryClientProvider>,
  );
}

// FIX-025 (D-158): the header never shows an endless "Sandbox starting": a failed start names the
// reason in the pill's tooltip, and the Computer tab shows the last start attempt.
describe("agent header sandbox pill (FIX-025)", () => {
  it("shows 'Sandbox starting' while starting and the failure reason on failed", async () => {
    renderPage(routes({ state: "starting" }));
    expect(await screen.findByText("Sandbox starting")).toBeTruthy();
    cleanup();
    renderPage(
      routes({
        state: "failed",
        failure_code: "supervisor_unreachable",
        failure_message:
          "The agent's computer started, but its supervisor could not reach the runner at aurora-runner.aurora (10.0.0.9):4105 within 60 s.",
      }),
    );
    const pill = await screen.findByText("Sandbox failed");
    expect(pill.getAttribute("title")).toContain("aurora-runner.aurora (10.0.0.9):4105");
    expect(pill.getAttribute("title")).toContain("supervisor_unreachable");
  });

  it("the Computer tab shows the last start attempt: phase, detail, elapsed and events", async () => {
    renderPage(
      routes({
        state: "failed",
        failure_code: "supervisor_unreachable",
        failure_message: "The agent's computer started, but its supervisor could not reach the runner.",
        start_attempt: {
          started_at: "2026-11-10T10:00:00Z",
          phase: "booting",
          detail: "waiting for the supervisor to connect to the runner at aurora-runner.aurora (10.0.0.9):4105",
          events: ["BackOff: restarting failed container"],
          progress_at: "2026-11-10T10:01:05Z",
        },
      }),
    );
    fireEvent.click(await screen.findByRole("tab", { name: "Computer" }));
    expect(await screen.findByText("Last start attempt")).toBeTruthy();
    expect(screen.getByText(/booting/)).toBeTruthy();
    expect(screen.getByText(/aurora-runner.aurora \(10.0.0.9\):4105/)).toBeTruthy();
    expect(screen.getByText("65 s after start")).toBeTruthy();
    expect(screen.getByText("BackOff: restarting failed container")).toBeTruthy();
    // The failure notice carries the human reason too.
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("could not reach the runner");
  });
});
