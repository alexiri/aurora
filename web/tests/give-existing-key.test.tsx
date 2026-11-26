import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { GiveKeyButton } from "@/components/agents/answer-fix";
import { UndoToastHost } from "@/components/ui/undo-toast";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// (FIX-249, C-1168) The quiet "Use another key" control beside the one-click give: one shared
// component for the header, the empty chat and the starter-key card. The other usable keys grant in
// one click with the same toast; the keys that cannot be used are disabled rows with the reason.

const GIVE = { action: "give_key", key: { secret_id: "k1", name: "Company key", scope: "organization" } } as const;

const key = (id: string, name: string, scope: string, over: Record<string, unknown> = {}) => ({
  secret_id: id, name, scope, starter: false, usable: true, reason: null, ...over,
});

const view = () =>
  wrap(
    <>
      <GiveKeyButton agentId="a1" agentName="Rex" fix={GIVE} />
      <UndoToastHost />
    </>,
  );

describe("Use another key (FIX-249)", () => {
  it("one key the agent does not have: the one-click button alone, no extra control", async () => {
    // The route lists every key the agent does not have yet — the fixed one included; with only it
    // in the list there is nothing "another" to offer.
    const fn = mockFetch({ "GET /v1/agents/a1/ai-keys": () => json(200, { items: [key("k1", "Company key", "organization")] }) });
    view();
    expect(await screen.findByRole("button", { name: "Give Rex the Company key" })).toBeInTheDocument();
    await waitFor(() => expect(fn.mock.calls.length).toBeGreaterThanOrEqual(1));
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByRole("button", { name: "Use another key" })).toBeNull();
  });

  it("several usable keys: the menu lists the other keys with their scope in words; picking the second grants the second, with the toast", async () => {
    const fn = mockFetch({
      "GET /v1/agents/a1/ai-keys": () =>
        json(200, {
          items: [
            { ...key("k2", "Sandbox key", "workspace"), starter: true },
            key("k3", "Lab key", "organization"),
          ],
        }),
      "POST /v1/secrets/k3/grants": () => json(200, { items: [{ agent_id: "a1", status: "created" }] }),
    });
    view();
    // Radix menus open from the keyboard in jsdom, as in the other tests of a menu.
    fireEvent.keyDown(await screen.findByRole("button", { name: "Use another key" }), { key: "Enter" });
    expect(await screen.findByRole("menuitem", { name: "Sandbox key Workspace key" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Lab key Organization key" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: "Lab key Organization key" }));
    await waitFor(() =>
      expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST" && new URL(r.url).pathname === "/v1/secrets/k3/grants")).toBe(true),
    );
    const post = fn.mock.calls.map(([r]) => r).find((r): r is Request => r instanceof Request && r.method === "POST")!;
    expect(await post.clone().json()).toEqual({ agent_ids: ["a1"] });
    expect(await screen.findByText("Rex can answer now")).toBeInTheDocument();
  });

  it("a key that cannot be used is a disabled row with its reason, and picking it grants nothing", async () => {
    const fn = mockFetch({
      "GET /v1/agents/a1/ai-keys": () =>
        json(200, {
          items: [
            key("k2", "Old key", "organization", { usable: false, reason: "Its service is not allowed here." }),
            key("k3", "Lab key", "organization"),
          ],
        }),
      "POST /v1/secrets/k2/grants": () => json(200, { items: [{ agent_id: "a1", status: "created" }] }),
    });
    view();
    // Radix menus open from the keyboard in jsdom, as in the other tests of a menu.
    fireEvent.keyDown(await screen.findByRole("button", { name: "Use another key" }), { key: "Enter" });
    const row = await screen.findByRole("menuitem", { name: "Old key Its service is not allowed here." });
    expect(row).toHaveAttribute("data-disabled");
    fireEvent.click(row);
    await new Promise((r) => setTimeout(r, 20));
    expect(fn.mock.calls.some(([r]) => r instanceof Request && r.method === "POST" && new URL(r.url).pathname === "/v1/secrets/k2/grants")).toBe(false);
  });

  it("a failed grant says so, and the menu stays", async () => {
    mockFetch({
      "GET /v1/agents/a1/ai-keys": () => json(200, { items: [key("k2", "Lab key", "organization")] }),
      "POST /v1/secrets/k2/grants": () =>
        json(403, { error: { code: "workspace_access_required", message: "Only a workspace owner can do that." } }),
    });
    view();
    // Radix menus open from the keyboard in jsdom, as in the other tests of a menu.
    fireEvent.keyDown(await screen.findByRole("button", { name: "Use another key" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Lab key Organization key" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Only a workspace owner can do that.");
  });
});
