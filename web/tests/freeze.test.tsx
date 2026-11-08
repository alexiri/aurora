import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { FreezeControl } from "@/components/freeze/freeze-panel";
import { AgentFreeze } from "@/components/freeze/freeze-sections";
import { authorityRank, type Frozen } from "@/lib/api/freeze";

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

const frozen = (over: Partial<Frozen> = {}): Frozen => ({
  at: "2026-01-02T03:04:05Z",
  by_label: "boss@example.com",
  reason: "compromised token",
  authority: "workspace_owner",
  level: "agent",
  ...over,
});

const noop = { mutateAsync: async () => undefined, isPending: false, error: null };

describe("authorityRank", () => {
  it("orders workspace_owner < org_admin < instance_admin", () => {
    expect(authorityRank("workspace_owner")).toBeLessThan(authorityRank("org_admin"));
    expect(authorityRank("org_admin")).toBeLessThan(authorityRank("instance_admin"));
  });
});

describe("FreezeControl", () => {
  it("requires a reason, lists the effects and sends the freeze", async () => {
    const freeze = vi.fn(async () => undefined);
    wrap(
      <FreezeControl
        subject="agent"
        name="Bot"
        frozen={null}
        callerRank={1}
        freeze={{ mutateAsync: freeze, isPending: false, error: null }}
        unfreeze={{ mutateAsync: async () => undefined, isPending: false, error: null }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Freeze agent" }));
    expect(await screen.findByText(/Running and queued turns are stopped/)).toBeInTheDocument();
    const confirm = screen.getByRole("button", { name: "Freeze agent", hidden: false, description: "" });
    expect(confirm).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "  stolen token  " } });
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);
    await waitFor(() => expect(freeze).toHaveBeenCalledWith("stolen token"));
  });

  it("shows the banner and an enabled Unfreeze when the caller's authority suffices", () => {
    wrap(<FreezeControl subject="agent" name="Bot" frozen={frozen()} callerRank={1} freeze={noop} unfreeze={noop} />);
    expect(screen.getByRole("status")).toHaveTextContent("Frozen by boss@example.com on");
    expect(screen.getByRole("status")).toHaveTextContent("compromised token");
    expect(screen.getByRole("button", { name: "Unfreeze agent" })).toBeEnabled();
  });

  it("disables Unfreeze with the explanation when a higher authority froze it", () => {
    wrap(
      <FreezeControl
        subject="agent"
        name="Bot"
        frozen={frozen({ authority: "org_admin" })}
        callerRank={authorityRank("workspace_owner")}
        freeze={noop}
        unfreeze={noop}
      />,
    );
    const b = screen.getByRole("button", { name: "Unfreeze agent" });
    expect(b).toBeDisabled();
    expect(screen.getByText(/A higher authority froze this/)).toBeInTheDocument();
  });

  it("offers no Unfreeze for a freeze inherited from the organization", () => {
    wrap(<FreezeControl subject="agent" name="Bot" frozen={frozen({ level: "org" })} callerRank={3} freeze={noop} unfreeze={noop} />);
    expect(screen.queryByRole("button", { name: /Unfreeze/ })).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("because its organization is frozen");
  });
});

describe("AgentFreeze", () => {
  it("posts the unfreeze for the agent", async () => {
    const fn = mockFetch({
      "GET /v1/orgs/o1": () => json(200, { id: "o1", my_role: "owner", settings: {} }),
      "POST /v1/agents/a1/unfreeze": () => json(200, { frozen: null, effective_frozen: false }),
    });
    wrap(<AgentFreeze agentId="a1" name="Bot" orgId="o1" frozen={frozen()} canOwn />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Unfreeze agent" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Unfreeze agent" }));
    await waitFor(() =>
      expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST" && r.url.endsWith("/v1/agents/a1/unfreeze"))).toBe(true),
    );
  });

  it("members without can_own see the banner but no controls", () => {
    wrap(<AgentFreeze agentId="a1" name="Bot" orgId="o1" frozen={frozen()} canOwn={false} />);
    expect(screen.getByRole("status")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });
});
