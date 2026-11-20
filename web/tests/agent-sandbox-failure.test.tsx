import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops", handle: "scout" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/agents/scout",
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("@/components/agents/chat-tab", () => ({ ChatTab: () => <div data-testid="chat-tab" /> }));
vi.mock("@/components/agents/instructions-tab", () => ({ InstructionsTab: () => <div data-testid="instructions-tab" /> }));
vi.mock("@/components/agents/delete-zone", () => ({ DeleteAgentDialog: () => null }));

import { AgentPage } from "@/components/agents/agent-page";
import { ComputerTab } from "@/components/agents/computer-tab";

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
  it("the header's one state follows the computer's readiness (D-408): Starting its computer…, never Ready until it answers, Can't answer when it failed", async () => {
    const computer = (over: Record<string, unknown>) => ({ word: "on", ready: true, working: false, asleep: false, phase: null, sleeps_in_secs: null, started_at: null, ...over });
    renderPage(routes({ state: "starting", computer: computer({ word: "starting", ready: false }) }));
    await waitFor(() => expect(screen.getByTestId("agent-state").textContent).toBe("Starting its computer…"));
    expect(screen.queryByText(/Computer: /)).toBeNull();
    cleanup();
    // Running, but the harness has not reported ready yet: the API says `starting`, so the header does too.
    renderPage(routes({ state: "running", computer: computer({ word: "starting", ready: false }) }));
    await waitFor(() => expect(screen.getByTestId("agent-state").textContent).toBe("Starting its computer…"));
    cleanup();
    renderPage(routes({ state: "running", computer: computer({}) }));
    await waitFor(() => expect(screen.getByTestId("agent-state").textContent).toBe("Ready"));
    cleanup();
    renderPage(routes({ state: "stopped", computer: computer({ word: "off", ready: false, asleep: true }) }));
    await waitFor(() => expect(screen.getByTestId("agent-state").textContent).toBe("Asleep"));
    cleanup();
    renderPage(
      routes({
        state: "failed", failure_code: "supervisor_unreachable", failure_message: "The agent's computer started, but its supervisor could not reach the runner.",
        computer: computer({ word: "failed", ready: false }),
      }),
    );
    await waitFor(() => expect(screen.getByTestId("agent-state").textContent).toBe("Can't answer: its computer couldn't start"));
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
    // The top card says it in words: the computer couldn't start, and why.
    expect(screen.getByTestId("computer-state").textContent).toBe("Scout's computer couldn't start");
    expect(screen.getByRole("region", { name: "Computer" }).textContent).toContain("could not reach the runner");
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

  it("shows Try again on a retryable failure, disabled as 'Start in flight…' once the retry answered", async () => {
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
    const btn = await screen.findByRole("button", { name: "Try again" });
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
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("hides Try again when the sandbox is not retryable (another state or frozen)", async () => {
    renderPage(routes({ state: "starting", retryable: false }));
    fireEvent.click(await screen.findByRole("tab", { name: "Computer" }));
    await screen.findByText("Start log");
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });

  const renderTab = (sandbox: Record<string, unknown>, frozen = false) => {
    mockFetch(routes(sandbox));
    return render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <ComputerTab agentId="a1" frozen={frozen} />
      </QueryClientProvider>,
    );
  };

  // (P5-POLISH-6, C5) The top card says the state in one sentence with the action that fits it: Restart only while awake
  // (the API answers 409 elsewhere, FIX-049), Try again after a failed start.
  it.each([
    ["stopping", "is asleep"],
    ["stopped", "is asleep"],
    ["unprovisioned", "is asleep"],
    ["starting", "is starting"],
  ])("offers no Restart when the computer is %s", async (state, words) => {
    renderTab({ state });
    expect((await screen.findByTestId("computer-state")).textContent).toContain(words);
    expect(screen.queryByRole("button", { name: "Restart" })).toBeNull();
  });

  it("offers Restart when the computer is awake", async () => {
    renderTab({ state: "running" });
    expect((await screen.findByTestId("computer-state")).textContent).toContain("is awake");
    expect((screen.getByRole("button", { name: "Restart" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("offers Try again after a failed start, and nothing for a frozen agent (the API would answer 423)", async () => {
    renderTab({ state: "failed", failure_code: "pod_failed", retryable: true });
    expect((await screen.findByTestId("computer-state")).textContent).toContain("couldn't start");
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
    cleanup();
    renderTab({ state: "failed", failure_code: "pod_failed", retryable: true }, true);
    await screen.findByTestId("computer-state");
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("says when the computer sleeps, in words, and nothing when idle stop is off (P5-APIGAP-1, C-1046)", async () => {
    renderTab({ state: "running", idle_stop_after_secs: 1800 });
    expect((await screen.findByRole("region", { name: "Computer" })).textContent).toContain("Sleeps after 30 minutes of quiet.");
    cleanup();
    renderTab({ state: "stopped", idle_stop_after_secs: 3600 });
    expect((await screen.findByRole("region", { name: "Computer" })).textContent).toContain("Sleeps after 1 hour of quiet.");
    cleanup();
    renderTab({ state: "stopped", idle_stop_after_secs: null });
    expect((await screen.findByRole("region", { name: "Computer" })).textContent).not.toContain("Sleeps after");
  });

  it("Wake it now posts the wake, stays pending until the status says starting, and is absent when awake or frozen (C-1045)", async () => {
    const calls: string[] = [];
    const base = routes({ state: "stopped" });
    mockFetch({
      ...base,
      "POST /v1/agents/a1/sandbox/wake": async () => {
        calls.push("wake");
        // 202: the start is requested, the view still says asleep until the runner claims it.
        return base["GET /v1/agents/a1/sandbox"]();
      },
    });
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <ComputerTab agentId="a1" />
      </QueryClientProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Wake it now" }));
    await waitFor(() => expect(calls).toEqual(["wake"]));
    const pending = await screen.findByRole("button", { name: "Waking…" });
    expect((pending as HTMLButtonElement).disabled).toBe(true);
    cleanup();
    renderTab({ state: "running" });
    await screen.findByTestId("computer-state");
    expect(screen.queryByRole("button", { name: "Wake it now" })).toBeNull();
    cleanup();
    renderTab({ state: "stopped" }, true);
    await screen.findByTestId("computer-state");
    expect(screen.queryByRole("button", { name: "Wake it now" })).toBeNull();
  });

  it("keeps the engine's details collapsed and says which pages are not set up, and who can turn them on", async () => {
    renderTab({ state: "stopped" });
    const details = (await screen.findByTestId("technical-details")) as HTMLDetailsElement;
    expect(details.open).toBe(false);
    expect(screen.getByTestId("missing-pages").textContent).toMatch(/Terminal, Files, Previews and SSH aren't set up on this Aurora\. An Aurora administrator can set this up\./);
  });
});
