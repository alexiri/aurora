import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { InstructionsTab } from "@/components/agents/instructions-tab";
import { ErrorBlock } from "@/components/chat/status-rows";
import type { Agent } from "@/lib/api/queries";

// (FIX-170, D-372, C-1026) The chat's `no_default_model` links open the failing agent's model field, and the
// Default model field says what the next turn uses while no model is chosen.

vi.mock("next/link", () => ({ default: ({ children, href }: { children: React.ReactNode; href: string }) => <a href={href}>{children}</a> }));
vi.mock("next/navigation", () => ({
  usePathname: () => "/o/acme/w/lab/agents/someone-else/chat/c9",
  useParams: () => ({ org: "acme", ws: "lab" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
}));

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.location.hash = "";
});

describe("no_default_model links", () => {
  it("opens the failing agent's Default model field, whatever page the chat is on, and drops the guide link", () => {
    render(<ErrorBlock code="no_default_model" message="The key serves no allowed chat model." base="/o/acme/w/lab" agentId="a-7" />);
    const choose = screen.getByRole("link", { name: "Choose a model" });
    expect(choose.getAttribute("href")).toBe("/o/acme/w/lab/agents/a-7?tab=instructions#a-model");
    expect(screen.queryByRole("link", { name: /guide/i })).toBeNull();
    // The server message names the cause.
    expect(screen.getByText("The key serves no allowed chat model.")).toBeTruthy();
  });

  it("keeps the guide link for a missing key, and the Agent access link goes to that agent too", () => {
    render(<ErrorBlock code="no_model_key" message="No LLM key is granted to this agent." base="/o/acme/w/lab" agentId="a-7" canOwn />);
    expect(screen.getByRole("link", { name: "Fix it in the guide" }).getAttribute("href")).toBe("/o/acme/w/lab/welcome?step=key");
    expect(screen.getByRole("link", { name: "Agent access" }).getAttribute("href")).toBe("/o/acme/w/lab/agents/a-7?tab=access");
  });
});

const agent = {
  id: "a1", org_id: "o1", workspace_id: "w1", handle: "bot", name: "Bot", kind: "hosted",
  is_default: false, harness: "claude-code", model_provider: null, model_endpoint_id: null, model_id: null,
  effort: null, proactive_messaging: "ask", brief: "", avatar_url: null, subagent_model: "inherit", model_roles: null,
  created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
} as unknown as Agent;

const view = (model_id: string | null, source: string, warning: string | null = null) => ({ model_id, source, warning });
function mock(readiness: unknown) {
  const routes: Record<string, unknown> = {
    "/v1/agents/a1/models": { items: [{ endpoint_id: "gw", model_id: "glm-5.3-flash-int4", provider: "litellm", dialects: [] }] },
    "/v1/harnesses": { default: "claude-code", items: [{ id: "claude-code", display_name: "Claude Code", available: true, dialects: [], model_roles: ["large", "medium", "small"] }] },
    "/v1/agents/a1/model-readiness": readiness,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const url = new URL(input instanceof Request ? input.url : String(input), "http://localhost");
      const body = routes[url.pathname];
      return body ? json(200, body) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}
const wrap = (ui: React.ReactNode) =>
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);

describe("Default model field", () => {
  it("shows what the next turn uses when the key's listing picked it, and each warning in one line", async () => {
    mock({
      harness: "claude-code", keys: [], override_dropped: false, warnings: ["default_model_not_served"],
      default: {
        endpoint_id: "gw", model_id: "glm-5.3-flash-int4", provider: "litellm", source: "key_listing",
        roles: { large: view(null, "none"), medium: view(null, "none"), small: view(null, "none") },
        roles_dropped: [], subagent_model: "inherit", subagent_warning: null,
      },
    });
    wrap(<InstructionsTab agent={agent} wsId="w1" />);
    await waitFor(() => expect(screen.getByTestId("model-automatic").textContent).toBe("Automatic: glm-5.3-flash-int4 (the first model your key serves)"));
    expect(screen.getByTestId("model-warning").textContent).toContain("not in your key's model list");
  });

  it("focuses the field when the link's #a-model fragment is present", async () => {
    window.location.hash = "#a-model";
    mock({ harness: "claude-code", keys: [], override_dropped: false, warnings: [], default: null });
    wrap(<InstructionsTab agent={agent} wsId="w1" />);
    await waitFor(() => expect(document.activeElement?.id).toBe("a-model"));
  });
});
