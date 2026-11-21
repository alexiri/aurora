import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { StarterKeyCard } from "@/components/agents/starter-key-card";
import { UndoToastHost } from "@/components/ui/undo-toast";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

const GIVE = { action: "give_key", key: { secret_id: "k1", name: "Company key", scope: "organization" } };
const answer = (over: Record<string, unknown> = {}) => ({ can_answer: false, reason: "no_ai_key", usable_keys: 0, fix: GIVE, ...over });

function routes(ready: unknown, post?: () => Response) {
  return {
    "GET /v1/agents/a1/answer-readiness": () => json(200, ready),
    "POST /v1/secrets/k1/grants": post ?? (() => json(200, { items: [{ agent_id: "a1", status: "created" }] })),
  };
}
const card = () =>
  wrap(
    <>
      <StarterKeyCard agentId="a1" agentName="Rex" />
      <UndoToastHost />
    </>,
  );

describe("the missing starter key (access-3)", () => {
  it("offers the organization's key by its own name and gives it in one click", async () => {
    const fn = mockFetch(routes(answer()));
    card();
    expect(await screen.findByText("Rex can't answer yet: it has no AI key.")).toBeInTheDocument();
    // True whether the agent never had the key or had it taken away; the card is the amber callout with a dot.
    expect(screen.getByTestId("starter-key-card").textContent).toContain("Your organization's Company key goes to every new agent automatically. Rex doesn't have it.");
    expect(screen.getByTestId("starter-key-card").textContent).not.toContain("was created before");
    expect(screen.getByTestId("starter-key-card").className).toContain("bg-pending-panel");
    expect(screen.getByRole("button", { name: "Choose another" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Give Rex the Company key" }));
    await waitFor(() => expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST" && new URL(r.url).pathname === "/v1/secrets/k1/grants")).toBe(true));
    const post = fn.mock.calls.map(([r]) => r).find((r): r is Request => r instanceof Request && r.method === "POST")!;
    expect(await post.clone().json()).toEqual({ agent_ids: ["a1"] });
    expect(await screen.findByText("Rex can answer now")).toBeInTheDocument();
  });

  it("shows nothing when the agent can answer, the key exists but stopped working, or the fix is another one", async () => {
    for (const r of [
      answer({ can_answer: true, reason: null, usable_keys: 1, fix: null }),
      answer({ reason: "key_not_usable" }),
      answer({ fix: { action: "add_key" } }),
      answer({ fix: { action: "ask_admin", who_can: { roles: ["organization admin"] } } }),
    ]) {
      const fn = mockFetch(routes(r));
      const { unmount } = card();
      await waitFor(() => expect(fn.mock.calls.length).toBeGreaterThanOrEqual(1));
      await new Promise((res) => setTimeout(res, 20));
      expect(screen.queryByTestId("starter-key-card")).toBeNull();
      unmount();
      vi.unstubAllGlobals();
    }
  });
});
