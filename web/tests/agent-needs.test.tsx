import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AgentsList } from "@/components/agents-list";
import { useAgentNeedsCounts } from "@/lib/api/agent-needs";
import { approvalKeys } from "@/lib/api/approvals";
import { actionApprovalKeys } from "@/lib/api/action-approvals";
import { json } from "./support/api";

// P5-REFINE-11 item 11 (P5-REFINE-20 G5): the agent's waiting approvals as a count on its row in the Agents list.

const agents = [
  { id: "a1", name: "Daneel", handle: "daneel", kind: "hosted", is_default: false },
  { id: "a2", name: "Rex", handle: "rex", kind: "hosted", is_default: false },
] as never[];

afterEach(() => vi.unstubAllGlobals());

describe("AgentsList needs count", () => {
  it("shows a count chip with screen-reader text on a row that has approvals waiting, none at zero or when not given", () => {
    const { rerender } = render(<AgentsList agents={agents} base="/o/x/w/y/agents" needs={{ a1: 2 }} />);
    const chip = screen.getByTestId("agent-needs-count");
    expect(chip).toHaveTextContent("2");
    expect(chip).toHaveAttribute("aria-label", "2 approvals waiting for you");
    expect(screen.getAllByTestId("agent-needs-count")).toHaveLength(1);
    rerender(<AgentsList agents={agents} base="/o/x/w/y/agents" needs={{ a1: 1, a2: 0 }} />);
    expect(screen.getByTestId("agent-needs-count")).toHaveAttribute("aria-label", "1 approval waiting for you");
    rerender(<AgentsList agents={agents} base="/o/x/w/y/agents" />);
    expect(screen.queryByTestId("agent-needs-count")).toBeNull();
  });
});

function Probe({ ws }: { ws: string }) {
  const { counts, canDecide } = useAgentNeedsCounts(ws);
  return <p data-testid="probe">{canDecide ? JSON.stringify(counts) : "cannot"}</p>;
}

function run(role: string, gateway: unknown[], actions: unknown[]) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const url = new URL(input instanceof Request ? input.url : String(input), "http://localhost");
      calls.push(url.pathname);
      if (url.pathname === "/v1/workspaces/w1") return json(200, { id: "w1", my_role: role });
      if (url.pathname === "/v1/workspaces/w1/approvals") return json(200, { items: gateway, next_cursor: null });
      if (url.pathname === "/v1/workspaces/w1/action-approvals") return json(200, { items: actions, next_cursor: null });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  render(
    <QueryClientProvider client={qc}>
      <Probe ws="w1" />
    </QueryClientProvider>,
  );
  return { qc, calls };
}

describe("useAgentNeedsCounts", () => {
  it("counts gateway and action approvals per agent for someone who can decide, and drops when they are decided", async () => {
    const { qc } = run("approver", [{ id: "g1", created_at: "2026-11-20T10:00:00Z", agent: { id: "a1" } }, { id: "g2", created_at: "2026-11-20T10:01:00Z", agent: { id: "a2" } }], [{ id: "x1", agent_id: "a1" }]);
    await waitFor(() => expect(screen.getByTestId("probe")).toHaveTextContent('{"a1":2,"a2":1}'));
    // Deciding refetches the lists (here: both are now empty) and the numbers go.
    qc.setQueryData(approvalKeys.pending("w1"), { items: [] });
    qc.setQueryData(actionApprovalKeys.pending("w1"), { items: [] });
    await waitFor(() => expect(screen.getByTestId("probe")).toHaveTextContent("{}"));
  });

  it("asks for nothing and shows nothing to a person who cannot decide", async () => {
    const { calls } = run("member", [{ id: "g1", created_at: "2026-11-20T10:00:00Z", agent: { id: "a1" } }], []);
    await waitFor(() => expect(screen.getByTestId("probe")).toHaveTextContent("cannot"));
    expect(calls.some((p) => p.endsWith("/approvals") || p.endsWith("/action-approvals"))).toBe(false);
  });
});
