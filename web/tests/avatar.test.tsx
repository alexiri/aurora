import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AgentsList } from "@/components/agents-list";
import { InstructionsTab } from "@/components/agents/instructions-tab";
import type { Agent } from "@/lib/api/queries";

vi.mock("next/link", () => ({ default: ({ children }: { children: React.ReactNode }) => <a>{children}</a> }));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

const base = {
  id: "a1", org_id: "o1", workspace_id: "w1", handle: "bot", name: "Bot", kind: "byo",
  is_default: false, harness: null, model_provider: null, model_endpoint_id: null, model_id: null,
  effort: null, proactive_messaging: "ask", brief: "", created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
} as const;
const withAvatar = { ...base, avatar_url: "/v1/public/avatars/KEY" } as Agent;
const without = { ...base, avatar_url: null } as Agent;

const calls: { method: string; path: string; body?: unknown; type?: string | null }[] = [];
function mockFetch(routes: Record<string, () => Response>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const method = req?.method ?? init?.method ?? "GET";
      calls.push({
        method,
        path: url.pathname,
        body: init?.body,
        type: new Headers(req?.headers ?? init?.headers).get("content-type"),
      });
      const h = routes[`${method} ${url.pathname}`];
      return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

beforeEach(() => {
  calls.length = 0;
  vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: () => "blob:preview", revokeObjectURL: () => {} }));
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const png = (size = 10) => new File([new Uint8Array(size)], "a.png", { type: "image/png" });

describe("avatar upload", () => {
  it("shows the current avatar and uploads the raw file with PUT", async () => {
    mockFetch({ "PUT /v1/agents/a1/avatar": () => json(200, withAvatar), "GET /v1/models": () => json(200, { items: [] }) });
    const { container } = wrap(<InstructionsTab agent={without} wsId="w1" />);
    expect(container.querySelector("img")).toBeNull();
    const file = png();
    fireEvent.change(screen.getByLabelText("Avatar file"), { target: { files: [file] } });
    await waitFor(() => expect(calls.some((c) => c.method === "PUT")).toBe(true));
    const put = calls.find((c) => c.method === "PUT")!;
    expect(put.path).toBe("/v1/agents/a1/avatar");
    expect(put.body).toBe(file);
    expect(put.type).toBe("application/octet-stream");
  });

  it("rejects the wrong type or size before any request", async () => {
    mockFetch({});
    wrap(<InstructionsTab agent={without} wsId="w1" />);
    const input = screen.getByLabelText("Avatar file");
    fireEvent.change(input, { target: { files: [new File(["<svg/>"], "a.svg", { type: "image/svg+xml" })] } });
    expect(await screen.findByRole("alert")).toHaveTextContent("PNG, JPEG or WebP");
    fireEvent.change(input, { target: { files: [png(512 * 1024 + 1)] } });
    expect(await screen.findByRole("alert")).toHaveTextContent("512 KiB");
    expect(calls.filter((c) => c.method === "PUT")).toHaveLength(0);
  });

  it("surfaces the server's avatar_invalid message", async () => {
    mockFetch({
      "PUT /v1/agents/a1/avatar": () =>
        json(422, { error: { code: "avatar_invalid", message: "The image could not be decoded." } }),
    });
    wrap(<InstructionsTab agent={without} wsId="w1" />);
    fireEvent.change(screen.getByLabelText("Avatar file"), { target: { files: [png()] } });
    expect(await screen.findByRole("alert")).toHaveTextContent("could not be decoded");
  });

  it("removes the avatar with DELETE", async () => {
    mockFetch({ "DELETE /v1/agents/a1/avatar": () => json(204, null) });
    const { container } = wrap(<InstructionsTab agent={withAvatar} wsId="w1" />);
    expect(container.querySelector("img")).toHaveAttribute("src", "/v1/public/avatars/KEY");
    fireEvent.click(screen.getByRole("button", { name: "Remove avatar" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/agents/a1/avatar")).toBe(true));
  });
});

describe("agents list", () => {
  it("renders the avatar image, or an initial when there is none", () => {
    const { container } = render(<AgentsList agents={[withAvatar, { ...without, id: "a2", name: "Zed" }]} />);
    expect(container.querySelector("img")).toHaveAttribute("src", "/v1/public/avatars/KEY");
    expect(screen.getByText("Z")).toBeInTheDocument();
  });
});
