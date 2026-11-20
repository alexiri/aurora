import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ComputerStateLine } from "@/components/agents/computer-state";
import { ComputerTab } from "@/components/agents/computer-tab";
import { seedMoreControls } from "./support/render";
import { json } from "./support/api";

// (P5-REFINE-15, D-408, D-412, SEC-141) The Computer's state line above every sub-page (the five words, the actions by
// altitude) and Details by role: a member reads "Started 23:55 · running" and nothing about the machinery.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops", handle: "dani" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/agents/dani",
  useSearchParams: () => new URLSearchParams(),
}));

afterEach(cleanup);

const AGENT = "a1";
const computer = (over: Record<string, unknown> = {}) => ({ word: "on", ready: true, working: false, asleep: false, phase: null, sleeps_in_secs: null, started_at: "2026-11-20T23:55:00Z", ...over });
const view = (over: Record<string, unknown> = {}) => ({
  agent_id: AGENT, state: "running", boot_id: "b1", runner_id: null, image: null, started_at: "2026-11-20T23:55:00Z",
  keep_awake_reasons: [], failure_code: null, failure_message: null, start_attempt: null, retryable: false, start_log: [],
  processes: [], ports: [], restart_to_update: false, idle_stop_after_secs: 1200, computer: computer(), ...over,
});

type Handler = (req?: Request) => Response | Promise<Response>;
function serve(sandbox: () => Record<string, unknown>, extra: Record<string, Handler> = {}, me: Record<string, unknown> = { is_instance_admin: false }) {
  const routes: Record<string, Handler> = {
    "GET /v1/agents/a1/sandbox": () => json(200, sandbox()),
    "GET /v1/me": () => json(200, { user: { id: "u1", display_name: "Ada", email: "a@b.c", ...me }, memberships: [], groups: [], approval_email: true }),
    "GET /v1/instance": () => json(200, { version: "0", features: { hosted: true, channels: false, previews: false, terminal: true, ssh: true } }),
    "GET /v1/agents/a1/start-failures": () => json(200, { items: [] }),
    ...extra,
  };
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

function mount(ui: React.ReactNode, more = true) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seedMoreControls(qc, more);
  return { qc, ...render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>) };
}

describe("the computer's state line", () => {
  const line = async () => (await screen.findByTestId("computer-state-words")).textContent ?? "";

  it("says the five phases in words, never the raw state", async () => {
    serve(() => view({ computer: computer({ sleeps_in_secs: null }) }));
    mount(<ComputerStateLine agentId={AGENT} agentName="Dani" />);
    expect(await line()).toBe("Dani's computer is on · sleeps after 20 minutes of quiet");
    cleanup();
    serve(() => view({ state: "stopped", computer: computer({ word: "off", ready: false, asleep: true }) }));
    mount(<ComputerStateLine agentId={AGENT} agentName="Dani" />);
    expect(await line()).toBe("Dani's computer is off · it starts when Dani needs it");
    cleanup();
    serve(() => view({ state: "starting", computer: computer({ word: "starting", ready: false }) }));
    mount(<ComputerStateLine agentId={AGENT} agentName="Dani" />);
    expect(await line()).toBe("Starting Dani's computer… Usually under 20 seconds.");
    cleanup();
    serve(() => view({ state: "stopping", computer: computer({ word: "stopping", ready: false }) }));
    mount(<ComputerStateLine agentId={AGENT} agentName="Dani" />);
    expect(await line()).toBe("Stopping…");
    cleanup();
    serve(() => view({ state: "failed", failure_code: "pod_failed", failure_message: "The pod crashed.", retryable: true, computer: computer({ word: "failed", ready: false }) }));
    mount(<ComputerStateLine agentId={AGENT} agentName="Dani" />);
    expect(await line()).toContain("Dani's computer couldn't start");
    expect(screen.getByRole("button", { name: "Try again" })).toBeEnabled();
    for (const raw of ["waiting_supervisor", "unprovisioned", "stopped"]) expect(document.body.textContent).not.toContain(raw);
  });

  it("counts 'sleeps in' while idle (C-1059) and follows a change from starting to on", async () => {
    let now = view({ state: "starting", computer: computer({ word: "starting", ready: false }) });
    serve(() => now);
    const { qc } = mount(<ComputerStateLine agentId={AGENT} agentName="Dani" />);
    expect(await line()).toContain("Starting Dani's computer");
    now = view({ computer: computer({ sleeps_in_secs: 20 }) });
    await act(async () => {
      qc.setQueryData(["agents", AGENT, "sandbox"], now);
    });
    await waitFor(async () => expect(await line()).toBe("Dani's computer is on · sleeps in 20 s when idle"));
  });

  it("offers Wake it now, Sleep now and Restart from Set up up, and none of them at Use", async () => {
    serve(() => view({ state: "stopped", computer: computer({ word: "off", ready: false, asleep: true }) }));
    mount(<ComputerStateLine agentId={AGENT} agentName="Dani" />, false);
    await line();
    expect(screen.queryByRole("button", { name: /Wake it now|Sleep now|Restart/ })).toBeNull();
    cleanup();
    serve(() => view({ state: "stopped", computer: computer({ word: "off", ready: false, asleep: true }) }));
    mount(<ComputerStateLine agentId={AGENT} agentName="Dani" />, true);
    expect(await screen.findByRole("button", { name: "Wake it now" })).toBeInTheDocument();
    cleanup();
    serve(() => view());
    mount(<ComputerStateLine agentId={AGENT} agentName="Dani" />, true);
    expect(await screen.findByRole("button", { name: "Sleep now" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restart" })).toBeInTheDocument();
  });

  it("Sleep now asks the API to sleep, without a confirmation", async () => {
    const fn = serve(() => view(), { "POST /v1/agents/a1/sandbox/sleep": () => json(202, view({ state: "stopping", computer: computer({ word: "stopping" }) })) });
    mount(<ComputerStateLine agentId={AGENT} agentName="Dani" />);
    fireEvent.click(await screen.findByRole("button", { name: "Sleep now" }));
    await waitFor(() => expect(fn.mock.calls.some(([r, i]) => (r instanceof Request ? r.method : (i as RequestInit | undefined)?.method) === "POST" && String(r instanceof Request ? r.url : r).endsWith("/sandbox/sleep"))).toBe(true));
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("Details by role (D-412, SEC-141)", () => {
  const machinery = view({
    image: "aurora/agent:2026.11", runner_id: "runner-1 · 10.42.0.17",
    computer: computer({ phase: "waiting_supervisor" }),
    start_attempt: { started_at: "2026-11-20T23:55:00Z", phase: "booting", detail: "cluster text 10.42.0.17", events: [], progress_at: null },
  });

  it("a member's DOM has no image, runner, address or raw phase, even if the API view carried them", async () => {
    serve(() => machinery);
    mount(<ComputerTab agentId={AGENT} agentName="Dani" terminal />, false);
    expect((await screen.findByTestId("computer-started")).textContent).toMatch(/^Started .+ · running$/);
    for (const secret of ["aurora/agent:2026.11", "runner-1", "10.42.0.17", "waiting_supervisor", "Admin only"]) expect(document.body.textContent).not.toContain(secret);
  });

  it("a member with the switch on reads the raw phase as a secondary line", async () => {
    serve(() => view({ computer: computer({ phase: "ready" }) }));
    mount(<ComputerTab agentId={AGENT} agentName="Dani" terminal />, true);
    expect((await screen.findByTestId("computer-phase")).textContent).toBe("ready");
    expect(screen.queryByText("Admin only")).toBeNull();
  });

  it("an instance admin gets the Admin only fold with image, runner and state", async () => {
    serve(() => machinery, {}, { is_instance_admin: true });
    mount(<ComputerTab agentId={AGENT} agentName="Dani" terminal />);
    expect(await screen.findByText("Admin only")).toBeInTheDocument();
    const facts = await screen.findByTestId("admin-facts");
    expect(facts.textContent).toContain("aurora/agent:2026.11");
    expect(facts.textContent).toContain("runner-1 · 10.42.0.17");
    expect(facts.textContent).toContain("running · waiting_supervisor");
  });

  it("follows the poll: a computer that woke no longer reads starting", async () => {
    let now = view({ state: "starting", started_at: null, computer: computer({ word: "starting", ready: false, started_at: null }) });
    serve(() => now);
    const { qc } = mount(<ComputerTab agentId={AGENT} agentName="Dani" terminal />, false);
    expect((await screen.findByTestId("computer-started")).textContent).toBe("starting");
    now = view();
    await act(async () => {
      qc.setQueryData(["agents", AGENT, "sandbox"], now);
    });
    await waitFor(() => expect(screen.getByTestId("computer-started").textContent).toMatch(/running$/));
  });
});
