import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/runners",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { AdminRunnersView } from "@/components/admin/runners-view";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const runner = {
  id: "r1",
  name: "runner-a",
  installation_fingerprint: "abcdef0123456789",
  backend: "kubernetes",
  capabilities: null,
  last_registered_at: "2026-11-10T10:00:00Z",
  created_at: "2026-11-01T10:00:00Z",
  live_replicas: 1,
  sandboxes: 2,
  running_sandboxes: 1,
  max_sandboxes: 4,
  replicas: [],
};

describe("admin runners view (FIX-025, C-586)", () => {
  it("lists runners and shows recent start failures with code, reason, phase and events", async () => {
    mockFetch({
      "GET /v1/admin/runners": () =>
        json(200, {
          items: [runner],
        }),
      "GET /v1/admin/runners/r1/start-failures": () =>
        json(200, {
          items: [
            {
              id: "f1",
              sandbox_id: "sb1",
              agent_id: "ag1",
              code: "supervisor_unreachable",
              message:
                "The agent's computer started, but its supervisor could not reach the runner at aurora-runner.aurora (10.0.0.9):4105 within 60 s. Check NetworkPolicy/CNI egress from the sandboxes namespace to the runner on port 4105 (Cilium: networkPolicy.cilium.enabled).",
              phase: "booting",
              events: ["FailedSandboxHello: no connection within 60s"],
              elapsed_secs: 64,
              created_at: "2026-11-10T09:58:00Z",
            },
          ],
        }),
    });
    wrap(<AdminRunnersView />);
    // The runner is named in the capacity table and in its card.
    expect((await within(document.body).findAllByText("runner-a")).length).toBe(2);
    expect(screen.getAllByText("kubernetes").length).toBe(2);
    expect(screen.getAllByText("1 live").length).toBe(2);
    fireEvent.click(screen.getByRole("button", { name: "Start failures" }));
    const code = await screen.findByText("supervisor_unreachable");
    expect(code).toBeTruthy();
    expect(screen.getByText(/aurora-runner.aurora \(10.0.0.9\):4105/)).toBeTruthy();
    expect(screen.getByText("booting")).toBeTruthy();
    expect(screen.getByText("64 s")).toBeTruthy();
    fireEvent.click(screen.getByText("Cluster events"));
    expect(screen.getByText("FailedSandboxHello: no connection within 60s")).toBeTruthy();
  });

  it("says when a runner has no start failures", async () => {
    mockFetch({
      "GET /v1/admin/runners": () => json(200, { items: [runner] }),
      "GET /v1/admin/runners/r1/start-failures": () => json(200, { items: [] }),
    });
    wrap(<AdminRunnersView />);
    fireEvent.click(await screen.findByRole("button", { name: "Start failures" }));
    expect(await screen.findByText("No start failures in the last 30 days.")).toBeTruthy();
  });

  // FIX-029 (C-614/C-615): a failure row names the agent; clicking it opens the **Start
  // diagnostics** drawer with that agent's history (runner named per row).
  it("opens the Start diagnostics drawer for an agent's failures from a row click", async () => {
    const fetchMock = mockFetch({
      "GET /v1/admin/runners": () => json(200, { items: [runner] }),
      "GET /v1/admin/runners/r1/start-failures": () =>
        json(200, {
          items: [
            {
              id: "f1", sandbox_id: "sb1", agent_id: "ag1", runner_id: "r1",
              code: "pod_failed", message: "the pod crashed", phase: "failed",
              events: [], elapsed_secs: 12, created_at: "2026-11-10T09:58:00Z",
            },
          ],
        }),
      "GET /v1/admin/agents/ag1/start-failures": () =>
        json(200, {
          items: [
            {
              id: "g2", sandbox_id: "sb1", agent_id: "ag1", runner_id: "r1",
              code: "pod_failed", message: "the pod crashed again", phase: "failed",
              events: [], elapsed_secs: 9, created_at: "2026-11-10T10:05:00Z",
            },
            {
              id: "g1", sandbox_id: "sb0", agent_id: "ag1", runner_id: null,
              code: "supervisor_unreachable", message: "no hello within 60 s", phase: "booting",
              events: [], elapsed_secs: 64, created_at: "2026-11-10T09:00:00Z",
            },
          ],
        }),
    });
    wrap(<AdminRunnersView />);
    fireEvent.click(await screen.findByRole("button", { name: "Start failures" }));
    // The row names the agent (its id).
    expect(await screen.findByText("ag1")).toBeTruthy();
    fireEvent.click(screen.getByText("ag1"));
    const dlg = await screen.findByRole("dialog");
    expect(dlg.textContent).toContain("Start diagnostics");
    await within(dlg).findByText("the pod crashed again");
    expect(dlg.textContent).toContain("supervisor_unreachable");
    expect(dlg.textContent).toContain("no hello within 60 s");
    // The drawer's rows name the runner (C-614); a deleted runner's row shows an empty cell.
    expect(dlg.textContent).toContain("r1");
    // FIX-100: the diagnostics query never fires with an empty agent segment (before a row is opened).
    const urls = fetchMock.mock.calls.map((c) => new URL((c[0] as Request).url, "http://localhost").pathname);
    expect(urls.some((u) => u.includes("/agents//"))).toBe(false);
    expect(urls).toContain("/v1/admin/agents/ag1/start-failures");
  });

  // FIX-121 (C-694): the capacity table shows what each runner runs against its limit.
  it("shows each runner's capacity, replicas, version and last seen", async () => {
    mockFetch({
      "GET /v1/admin/runners": () =>
        json(200, {
          items: [
            { ...runner, replicas: [{ version: "0.9.1", live: true, last_seen_at: "2026-11-10T10:05:00Z" }] },
            { ...runner, id: "r2", name: "runner-b", max_sandboxes: null, running_sandboxes: 3, live_replicas: 0, backend: null },
          ],
        }),
    });
    wrap(<AdminRunnersView />);
    const table = await screen.findByRole("table", { name: "Runner capacity" });
    expect(within(table).getByText("1 of 4 running")).toBeTruthy();
    expect(within(table).getByText("0.9.1")).toBeTruthy();
    expect(within(table).getByText("3 running (no limit declared)")).toBeTruthy();
    expect(within(table).getByText("not registered")).toBeTruthy();
    expect(within(table).getByText("never")).toBeTruthy();
  });

  // FIX-121 (C-695): confirm, choose the grace, the new token is shown once, the old one's end is told.
  it("rotates a runner's token with a grace period and shows the new token once", async () => {
    let called = "";
    mockFetch({
      "GET /v1/admin/runners": () => json(200, { items: [runner] }),
      "POST /v1/admin/runners/r1/token": (req) => {
        called = new URL(req.url, "http://localhost").search;
        return json(200, {
          token: "aur_NEWTOKEN",
          runner: { ...runner, previous_token_expires_at: "2026-11-10T10:20:00Z" },
        });
      },
    });
    wrap(<AdminRunnersView />);
    fireEvent.click(await screen.findByRole("button", { name: "Rotate token" }));
    const dlg = await screen.findByRole("dialog");
    fireEvent.change(within(dlg).getByLabelText("Grace period for the old token"), { target: { value: "3600" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Rotate token" }));
    await waitFor(() => expect(screen.getByTestId("secret-key").textContent).toBe("aur_NEWTOKEN"));
    expect(called).toBe("?grace_secs=3600");
    expect(screen.getByRole("dialog").textContent).toContain("The old token keeps working until");
  });

  it("marks an offline runner", async () => {
    mockFetch({
      "GET /v1/admin/runners": () => json(200, { items: [{ ...runner, live_replicas: 0 }] }),
    });
    wrap(<AdminRunnersView />);
    expect(await screen.findByText("offline")).toBeTruthy();
    await waitFor(() => expect(screen.queryByRole("button", { name: "Start failures" })).toBeTruthy());
  });
});
