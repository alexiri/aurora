import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { StarterKeyCard } from "@/components/agents/starter-key-card";
import { UndoToastHost } from "@/components/ui/undo-toast";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

const starterKey = (over: Record<string, unknown> = {}) => ({
  ref: { type: "secret", id: "k1" }, kind: "ai_key", name: "Company key", scope: "organization", starter: true, ...over,
});
const readiness = (keys: unknown[], def: unknown = null) => ({ harness: "h", override_dropped: false, default: def, keys });

function routes(opts: { accounts?: unknown[]; ready?: unknown; post?: () => Response; role?: string }) {
  return {
    "GET /v1/orgs/o1": () => json(200, { id: "o1", my_role: opts.role ?? "owner", settings: {} }),
    "GET /v1/orgs/o1/accounts": () => json(200, { items: opts.accounts ?? [starterKey()], next_cursor: null }),
    "GET /v1/agents/a1/model-readiness": () => json(200, opts.ready ?? readiness([])),
    "POST /v1/secrets/k1/grants": opts.post ?? (() => json(200, { items: [{ agent_id: "a1", status: "created" }] })),
  };
}
const card = () =>
  wrap(
    <>
      <StarterKeyCard agentId="a1" agentName="Rex" orgId="o1" />
      <UndoToastHost />
    </>,
  );

describe("the missing starter key (access-3)", () => {
  it("offers the organization's key by its own name and gives it in one click", async () => {
    const fn = mockFetch(routes({}));
    card();
    expect(await screen.findByText("Rex can't answer yet: it has no AI key.")).toBeInTheDocument();
    expect(screen.getByText(/Your organization's Company key goes to every new agent automatically, and Rex was created before it was set\./)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Choose another" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Give Rex the Company key" }));
    await waitFor(() => expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST" && new URL(r.url).pathname === "/v1/secrets/k1/grants")).toBe(true));
    const post = fn.mock.calls.map(([r]) => r).find((r): r is Request => r instanceof Request && r.method === "POST")!;
    expect(await post.clone().json()).toEqual({ agent_ids: ["a1"] });
    expect(await screen.findByText("Rex can answer now")).toBeInTheDocument();
  });

  it("never asks for the organization's accounts of someone who is not an owner or admin (403 in the console)", async () => {
    const fn = mockFetch(routes({ role: "member" }));
    card();
    await waitFor(() => expect(fn.mock.calls.some(([r]) => r instanceof Request && new URL(r.url).pathname === "/v1/orgs/o1")).toBe(true));
    await new Promise((res) => setTimeout(res, 30));
    expect(fn.mock.calls.some(([r]) => r instanceof Request && new URL(r.url).pathname === "/v1/orgs/o1/accounts")).toBe(false);
    expect(screen.queryByTestId("starter-key-card")).toBeNull();
  });

  it("shows nothing when the agent can answer, the key is already given, or the organization has no starter key", async () => {
    const usable = { secret_id: "k1", name: "Company key", scope: "organization", provider: "p", endpoint_id: "e", usable: true, reason: null };
    for (const r of [
      routes({ ready: readiness([usable], { model_id: "m" }) }),
      routes({ ready: readiness([{ ...usable, usable: false, reason: "inactive" }]) }),
      routes({ accounts: [starterKey({ starter: false })] }),
    ]) {
      const fn = mockFetch(r);
      const { unmount } = card();
      await waitFor(() => expect(fn.mock.calls.length).toBeGreaterThanOrEqual(2));
      await new Promise((res) => setTimeout(res, 20));
      expect(screen.queryByTestId("starter-key-card")).toBeNull();
      unmount();
      vi.unstubAllGlobals();
    }
  });
});
