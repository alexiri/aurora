import { cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AuditTab } from "@/components/settings/audit-tab";
import { wrap } from "./support/render";
import { json, mockFetch } from "./support/api";

// P5-REFINE-16 workspace-3: the workspace audit as sentences with names, Critical in red, the raw event under Details.

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(""),
  usePathname: () => "/o/acme/w/main/settings",
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const entry = (id: string, action: string, over: Record<string, unknown>) => ({
  id, org_id: "o1", workspace_id: "w1", actor_kind: "user", actor_id: "u1", actor_label: "Bob", action,
  target_kind: null, target_id: null, outcome: "success", source: "api", ip: null, metadata: {}, critical: false,
  created_at: "2026-11-20T14:02:00Z", ...over,
});

describe("workspace audit sentences", () => {
  it("names the person and the key, colours Critical red, and keeps the raw action under Details", async () => {
    mockFetch({
      "GET /v1/workspaces/w1/audit": () =>
        json(200, {
          items: [
            entry("1", "api_key.revoke", { critical: true, target_kind: "api_key", target_id: "k-gone", metadata: { name: "ci-script" } }),
            entry("2", "workspace_access.set", { target_kind: "user", target_id: "u2", metadata: { role: "owner" } }),
          ],
          next_cursor: null,
        }),
      "GET /v1/workspaces/w1/access": () =>
        json(200, { items: [{ user_id: "u2", display_name: "Carol", email: "carol@x.io", role: "owner", status: "active" }], next_cursor: null }),
      "GET /v1/workspaces/w1/api-keys": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/workspaces/w1/agents": () => json(200, { items: [], next_cursor: null }),
    });
    wrap(<AuditTab wsId="w1" />);
    expect(await screen.findByText("Bob revoked the API key ci-script")).toBeInTheDocument();
    expect(await screen.findByText("Bob changed Carol to Owner")).toBeInTheDocument();
    const critical = screen.getByText("Critical");
    expect(critical.className).toMatch(/danger/);
    expect(screen.getByText("Important").className).toMatch(/pending/);
    expect(screen.queryByText("api_key.revoke")).toBeNull();
    fireEvent.click(screen.getAllByRole("button", { name: "Details" })[0]!);
    expect(await screen.findByText("api_key.revoke")).toBeInTheDocument();
  });
});
