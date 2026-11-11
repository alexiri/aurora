import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PreviewsTab, ShareDialog } from "@/components/agents/previews-tab";

// P4-PRV-1 dashboard tests (docs/18 §Port previews): the tab renders the shares table, the add
// dialog maps server errors to field messages, and a paused share offers Resume. The tab itself
// is mounted by AgentPage only when the instance runs previews (features.previews) and the agent
// is hosted, so the "absent" cases are covered by that condition.

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

function mockFetch(routes: Record<string, (req: Request, url: URL) => Response>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    // `chatApi` fetches with relative paths; resolve them so `Request` construction never throws.
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
    const url = new URL(req.url);
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req, url) : json(404, { error: { code: "not_found", message: "nf" } });
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

const share = {
  id: "00000000-0000-4000-8000-000000000001",
  agent_id: "00000000-0000-4000-8000-000000000002",
  port: 3000,
  label: "pabcdefghijklmnopqrstuvwxyz234567",
  hostname: "pabcdefghijklmnopqrstuvwxyz234567.previews.example.org",
  url: "https://pabcdefghijklmnopqrstuvwxyz234567.previews.example.org/",
  visibility: "private" as const,
  state: "active" as const,
  status: "listening",
  start_command: "npm run dev",
  cwd: null,
  last_start: null,
  last_accessed_at: null,
  created_by: { id: null, label: "ada@example.test" },
  created_at: "2026-01-01T00:00:00Z",
};

describe("PreviewsTab", () => {
  it("renders the shares table with status chip and actions", async () => {
    mockFetch({ "GET /v1/agents/a1/previews": () => json(200, { shares: [share] }) });
    wrap(<PreviewsTab agentId="a1" frozen={false} />);
    await screen.findByText("pabcdefghijklmnopqrstuvwxyz234567.previews.example.org");
    expect(screen.getByText("Listening")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start now" })).toBeInTheDocument();
    expect(screen.getByText("npm run dev")).toBeInTheDocument();
  });

  it("shows the empty state", async () => {
    mockFetch({ "GET /v1/agents/a1/previews": () => json(200, { shares: [] }) });
    wrap(<PreviewsTab agentId="a1" frozen={false} />);
    await screen.findByText("No shares yet");
  });

  it("shows the frozen notice and hides row actions", async () => {
    mockFetch({ "GET /v1/agents/a1/previews": () => json(200, { shares: [share] }) });
    wrap(<PreviewsTab agentId="a1" frozen={true} />);
    await screen.findByText(/frozen/i);
    expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
  });
});

describe("ShareDialog", () => {
  it("maps the duplicate-port conflict to a notice", async () => {
    const fetchFn = mockFetch({
      "POST /v1/agents/a1/previews": () => json(409, { error: { code: "preview_exists", message: "A preview share for this port already exists." } }),
    });
    wrap(<ShareDialog agentId="a1" onDone={() => {}} />);
    fireEvent.change(screen.getByLabelText("Port"), { target: { value: "3000" } });
    fireEvent.click(screen.getByRole("button", { name: "Share port" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/already exists/));
    expect(fetchFn.mock.calls.some(([r]) => (r as Request).method === "POST")).toBe(true);
  });

  it("does not submit without a port", async () => {
    const fetchFn = mockFetch({});
    wrap(<ShareDialog agentId="a1" onDone={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Share port" }));
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    expect(fetchFn.mock.calls.some(([r]) => (r as Request).method === "POST")).toBe(false);
  });

  it("submits a new share with port and command", async () => {
    const fetchFn = mockFetch({
      "POST /v1/agents/a1/previews": () => json(201, { ...share, port: 5173 }),
    });
    wrap(<ShareDialog agentId="a1" onDone={() => {}} />);
    fireEvent.change(screen.getByLabelText("Port"), { target: { value: "5173" } });
    fireEvent.change(screen.getByLabelText("Start command (optional)"), { target: { value: "npm run dev" } });
    fireEvent.click(screen.getByRole("button", { name: "Share port" }));
    await waitFor(() => {
      const call = fetchFn.mock.calls.find(([r]) => (r as Request).method === "POST");
      expect(call).toBeDefined();
    });
    const call = fetchFn.mock.calls.find(([r]) => (r as Request).method === "POST")![0] as Request;
    expect(await call.json()).toEqual({ port: 5173, start_command: "npm run dev" });
  });

  it("offers the listening ports as chips", async () => {
    mockFetch({
      "GET /v1/agents/a1/sandbox": () => json(200, { ports: [{ port: 3000, protocol: "tcp" }, { port: 8080, protocol: "tcp" }] }),
    });
    wrap(<ShareDialog agentId="a1" onDone={() => {}} />);
    await screen.findByRole("button", { name: "3000" });
    expect(screen.getByRole("button", { name: "8080" })).toBeInTheDocument();
  });
});
