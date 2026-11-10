import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AdminIdentityView } from "@/components/identity/admin-identity-view";
import { IdpRoleBadge } from "@/components/identity/idp-role-badge";
import { IdpRolesList } from "@/components/identity/idp-roles-list";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("roles from IdP claims (P4-IDP-1)", () => {
  it("badges a role that comes from the IdP", () => {
    render(<IdpRoleBadge value="aurora-admins" />);
    expect(screen.getByText(/from idp role/i)).toHaveTextContent("From IdP role aurora-admins");
  });

  it("lists every claim value and what it granted, unmapped ones too", () => {
    render(
      <IdpRolesList
        roles={{
          provider: "cern",
          values: ["aurora-admins", "it-ai-team", "other"],
          synced_at: "2026-11-10T10:00:00Z",
          granted: [
            { value: "aurora-admins", instance_admin: true, org: null, workspaces: [] },
            {
              value: "it-ai-team",
              instance_admin: false,
              org: { slug: "it", role: "owner" },
              workspaces: [{ slug: "infra", role: "approver" }],
            },
          ],
        }}
      />,
    );
    expect(screen.getByText("Instance admin")).toBeInTheDocument();
    expect(screen.getByText(/owner in organization it; approver in workspace it\/infra/)).toBeInTheDocument();
    expect(screen.getByText("Grants nothing in Aurora")).toBeInTheDocument();
  });

  it("shows the mapping table read-only on Admin › Identity", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        json(200, {
          providers: [
            {
              id: "cern",
              display_name: "CERN SSO",
              issuer: "https://auth.example/realms/cern",
              role_claim: "cern_roles",
              role_claim_source: "id_token",
              require_role: true,
              role_mappings: [
                {
                  value: "aurora-admins",
                  instance_admin: true,
                  org: null,
                  workspaces: [],
                },
                { value: "aurora-users", instance_admin: false, org: { slug: "users", role: "member" }, workspaces: [] },
              ],
            },
          ],
        }),
      ),
    );
    wrap(<AdminIdentityView />);
    expect(await screen.findByText("cern_roles")).toBeInTheDocument();
    expect(screen.getByText("aurora-admins")).toBeInTheDocument();
    expect(screen.getByText("member in organization users")).toBeInTheDocument();
    expect(screen.getByText(/only people with a mapped role may sign in/i)).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });
});
