import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
import { ComputerTab } from "@/components/agents/computer-tab";

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
        retryable: false, start_log: [],
        processes: [], ports: [], restart_to_update: false, ...sandbox,
      }),
    "GET /v1/agents/a1/start-failures": () => json(200, { items: [] }),
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

// FIX-029 (C-611/C-612/C-615): the Computer tab renders the boot's start log under Last start
// attempt with a link to the failure history, and the failed notice gains **Retry now**.
describe("Computer tab start log and retry (FIX-029)", () => {
  it("renders the Start log (newest first) and the history link, empty state included", async () => {
    renderPage(
      routes({
        state: "failed",
        failure_code: "pod_failed",
        failure_message: "the pod crashed",
        retryable: true,
        start_log: [
          { ts: "2026-11-10T10:01:00Z", kind: "phase", phase: "failed", detail: "the pod crashed" },
          { ts: "2026-11-10T10:00:10Z", kind: "phase", phase: "pulling", detail: "pulling image" },
        ],
      }),
    );
    fireEvent.click(await screen.findByRole("tab", { name: "Computer" }));
    expect(await screen.findByText("Start log")).toBeTruthy();
    expect(screen.getAllByText(/the pod crashed/).length).toBeGreaterThan(0);
    expect(screen.getByText(/pulling image/)).toBeTruthy();
    expect(screen.getByText("Recent start failures")).toBeTruthy();
  });

  it("shows Retry now on a retryable failure, disabled as 'Start in flight…' once the retry answered", async () => {
    const calls: string[] = [];
    // A deferred POST: the pending state is observable.
    let release: (v: Response) => void = () => undefined;    const rts: Record<string, Handler> = {
      ...routes({
        state: "failed",
        failure_code: "pod_failed",
        failure_message: "the pod crashed",
        retryable: true,
        start_log: [{ ts: "2026-11-10T10:01:00Z", kind: "phase", phase: "failed", detail: "the pod crashed" }],
      }),
      // A deferred POST: the pending state is observable.
      "POST /v1/agents/a1/sandbox/retry": () =>
        new Promise<Response>((res) => {
          calls.push("retry");
          release = res;
        }),
    };
    mockFetch(rts);
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <ComputerTab agentId="a1" />
      </QueryClientProvider>,
    );
    const btn = await screen.findByRole("button", { name: "Retry now" });
    fireEvent.click(btn);
    await waitFor(() => expect(calls).toContain("retry"));
    // While the POST is in flight the button is disabled ("Start in flight…", C-615).
    const inflight = await screen.findByRole("button", { name: "Start in flight…" });
    expect((inflight as HTMLButtonElement).disabled).toBe(true);
    // The 202 view parks the sandbox in `stopped`: the failed notice (and the button) go away.
    release(
      json(202, {
        agent_id: "a1", state: "stopped", boot_id: null, runner_id: null, image: null, started_at: null,
        keep_awake_reasons: [], failure_code: null, failure_message: null, start_attempt: null,
        retryable: false, start_log: [], processes: [], ports: [], restart_to_update: false,
      }),
    );
    await waitFor(() => expect(screen.queryByRole("button", { name: "Start in flight…" })).toBeNull());
    expect(screen.queryByRole("button", { name: "Retry now" })).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("hides Retry now when the sandbox is not retryable (another state or frozen)", async () => {
    renderPage(routes({ state: "starting", retryable: false }));
    fireEvent.click(await screen.findByRole("tab", { name: "Computer" }));
    await screen.findByText("Start log");
    expect(screen.queryByRole("button", { name: "Retry now" })).toBeNull();
  });
});
