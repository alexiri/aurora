import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { InstanceAdminLink } from "@/components/app-shell";

// FIX-026: instance admins reach the admin pages from the sidebar on every page.

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function me(isAdmin: boolean) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      if (url.pathname === "/v1/me") {
        return json(200, {
          user: { id: "u1", email: "alex@example.org", display_name: "Alex", is_instance_admin: isAdmin },
          default_org_id: null,
          approval_email: true,
          client_address: null,
          memberships: [],
        });
      }
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("InstanceAdminLink", () => {
  it("shows an Instance admin link to instance admins outside /admin", async () => {
    me(true);
    wrap(<InstanceAdminLink pathname="/o/acme/w/main/agents" />);
    const link = await screen.findByRole("link", { name: "Instance admin" });
    expect(link.getAttribute("href")).toBe("/admin/users");
  });

  it("is hidden for users who are not instance admins", async () => {
    me(false);
    wrap(<InstanceAdminLink pathname="/o/acme/w/main/agents" />);
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("link", { name: "Instance admin" })).toBeNull();
  });

  it("is hidden on the admin pages themselves (the admin nav is shown there)", async () => {
    me(true);
    wrap(<InstanceAdminLink pathname="/admin/connectors" />);
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("link", { name: "Instance admin" })).toBeNull();
  });
});
