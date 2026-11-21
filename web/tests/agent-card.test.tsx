import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AgentsList } from "@/components/agents-list";
import { AgentCard } from "@/components/agents/agent-card";
import { json } from "./support/api";

// P5-LAYOUT-3 (D-440): one AgentCard for Overview and the Agents page.

const daneel = { id: "a1", name: "Daneel", handle: "daneel", kind: "hosted", is_default: true } as never;
const rex = { id: "a2", name: "Rex", handle: "rex", kind: "byo", is_default: false, byo: { posture: "disabled", posture_level: "organization" } } as never;

afterEach(() => vi.unstubAllGlobals());

function inList(ui: React.ReactNode) {
  return render(<ul>{ui}</ul>);
}

describe("AgentCard", () => {
  it("shows the name link, the handle, the kind chip and Chat to the agent's page", () => {
    inList(<AgentCard agent={daneel} href="/o/x/w/y/agents/daneel" chat />);
    const link = screen.getByRole("link", { name: "Daneel" });
    expect(link).toHaveAttribute("href", "/o/x/w/y/agents/daneel");
    expect(link).toHaveAttribute("id", "agent-link-a1");
    expect(screen.getByText("@daneel")).toBeInTheDocument();
    expect(screen.getByText("Came with this workspace")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Chat with Daneel" })).toHaveAttribute("href", "/o/x/w/y/agents/daneel");
  });

  it("shows the needs-you chip above zero only, and 'BYO disabled by' on its condition", () => {
    const { rerender } = inList(<AgentCard agent={rex} href="/a/rex" needs={3} />);
    expect(screen.getByTestId("agent-needs-count")).toHaveAttribute("aria-label", "3 approvals waiting for you");
    expect(screen.getByText("BYO disabled by organization")).toBeInTheDocument();
    rerender(
      <ul>
        <AgentCard agent={daneel} href="/a/daneel" needs={0} />
      </ul>,
    );
    expect(screen.queryByTestId("agent-needs-count")).toBeNull();
    expect(screen.queryByText(/BYO disabled/)).toBeNull();
  });

  it("has no Chat unless asked, and no badges line when there is nothing to show", () => {
    inList(<AgentCard agent={{ ...(rex as object), byo: undefined, kind: "hosted" } as never} href="/a/rex" />);
    expect(screen.queryByRole("link", { name: /Chat with/ })).toBeNull();
    expect(screen.queryByText(/Came with|Bring your own|BYO disabled|Token/)).toBeNull();
  });

  it("adds the summary when the workspace and organization are given", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.stubGlobal("fetch", vi.fn(async () => json(404, { error: { code: "not_found", message: "nf" } })));
    render(
      <QueryClientProvider client={qc}>
        <ul>
          <AgentCard agent={daneel} href="/a/daneel" wsId="w1" orgId="o1" />
        </ul>
      </QueryClientProvider>,
    );
    // The summary mounts (its own queries load); the card still has its head.
    expect(within(screen.getByRole("listitem")).getByText("@daneel")).toBeInTheDocument();
  });
});

describe("AgentsList as cards", () => {
  it("is a list labelled Agents with one listitem per agent", () => {
    render(<AgentsList agents={[daneel, rex]} base="/o/x/w/y/agents" />);
    const list = screen.getByRole("list", { name: "Agents" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(2);
    expect(within(list).getByRole("link", { name: "Chat with Rex" })).toBeInTheDocument();
  });

  it("says so when there are no agents", () => {
    render(<AgentsList agents={[]} />);
    expect(screen.getByText("No agents yet")).toBeInTheDocument();
  });
});
