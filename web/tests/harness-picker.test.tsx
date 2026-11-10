import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { InstructionsTab } from "@/components/agents/instructions-tab";

// P4-HAR (D-122, docs/14 §Instructions): the harness picker.

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

function mockFetch(routes: Record<string, (req: Request) => Response | Promise<Response>>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
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

const agent = (kind: "byo" | "hosted", harness: string | null = "claude-code") => ({
  id: "a1", org_id: "o1", workspace_id: "w1", handle: "bot", name: "Bot", kind,
  is_default: false, harness, model_provider: null, model_endpoint_id: null, model_id: null,
  effort: null, proactive_messaging: "ask", brief: "", effective_frozen: false,
  created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
});

const claude = { id: "claude-code", display_name: "Claude Code", dialects: ["anthropic_messages"], available: true };
const codex = (available: boolean) => ({
  id: "codex", display_name: "OpenAI Codex", dialects: ["openai_responses"], available,
});

/** The picker replaces the read-only field once `/v1/harnesses` answers. */
async function picker(): Promise<HTMLSelectElement> {
  await waitFor(() => expect(screen.getByLabelText("Harness").tagName).toBe("SELECT"));
  return screen.getByLabelText("Harness") as HTMLSelectElement;
}

describe("Instructions harness picker", () => {
  it("is not shown when only one harness is enabled", async () => {
    mockFetch({ "GET /v1/harnesses": () => json(200, { items: [claude], default: "claude-code" }) });
    wrap(<InstructionsTab agent={agent("hosted")} wsId="w1" />);
    await waitFor(() => expect(screen.getByLabelText("Harness")).toBeInTheDocument());
    // Still the read-only field of phase 0-3.
    expect(screen.getByLabelText("Harness")).toBeDisabled();
    expect(screen.getByLabelText("Harness").tagName).toBe("INPUT");
  });

  it("is never shown for a BYO agent", () => {
    const fn = mockFetch({});
    wrap(<InstructionsTab agent={agent("byo", null)} wsId="w1" />);
    expect(screen.getByLabelText("Harness").tagName).toBe("INPUT");
    expect(fn.mock.calls.some(([r]) => String((r as Request).url).includes("/v1/harnesses"))).toBe(false);
  });

  it("lists the enabled harnesses and disables one no runner offers", async () => {
    mockFetch({ "GET /v1/harnesses": () => json(200, { items: [claude, codex(false)], default: "claude-code" }) });
    wrap(<InstructionsTab agent={agent("hosted")} wsId="w1" />);
    const select = await picker();
    expect(select.tagName).toBe("SELECT");
    const options = Array.from(select.options);
    expect(options.map((o) => o.textContent)).toEqual(["Claude Code", "OpenAI Codex (no runner offers this)"]);
    expect(options[0].disabled).toBe(false);
    expect(options[1].disabled).toBe(true);
    expect(select.value).toBe("claude-code");
  });

  it("asks for confirmation, then sends the new harness", async () => {
    const patched: unknown[] = [];
    mockFetch({
      "GET /v1/harnesses": () => json(200, { items: [claude, codex(true)], default: "claude-code" }),
      "PATCH /v1/agents/a1": async (req) => {
        patched.push(await req.json());
        return json(200, agent("hosted", "codex"));
      },
    });
    wrap(<InstructionsTab agent={agent("hosted")} wsId="w1" />);
    const select = await picker();
    fireEvent.change(select, { target: { value: "codex" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    // Nothing is sent before the person confirms.
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(
      "Starts fresh sessions for every conversation of this agent; the transcript is kept.",
    );
    expect(patched).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Change harness" }));
    await waitFor(() => expect(patched).toHaveLength(1));
    expect(patched[0]).toMatchObject({ harness: "codex", name: "Bot" });
    await screen.findByText("Saved");
  });

  it("does not send a harness when it is unchanged", async () => {
    const patched: Record<string, unknown>[] = [];
    mockFetch({
      "GET /v1/harnesses": () => json(200, { items: [claude, codex(true)], default: "claude-code" }),
      "PATCH /v1/agents/a1": async (req) => {
        patched.push((await req.json()) as Record<string, unknown>);
        return json(200, agent("hosted"));
      },
    });
    wrap(<InstructionsTab agent={agent("hosted")} wsId="w1" />);
    await picker();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(patched).toHaveLength(1));
    expect(patched[0]).not.toHaveProperty("harness");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows the server's refusal while a turn runs", async () => {
    mockFetch({
      "GET /v1/harnesses": () => json(200, { items: [claude, codex(true)], default: "claude-code" }),
      "PATCH /v1/agents/a1": () =>
        json(409, { error: { code: "turn_active", message: "The agent's harness cannot change while a turn is running." } }),
    });
    wrap(<InstructionsTab agent={agent("hosted")} wsId="w1" />);
    fireEvent.change(await picker(), { target: { value: "codex" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(await screen.findByRole("button", { name: "Change harness" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("cannot change while a turn is running");
  });
});
