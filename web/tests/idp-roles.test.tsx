import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { IdpRoleBadge } from "@/components/identity/idp-role-badge";
import { IdpRolesList } from "@/components/identity/idp-roles-list";

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
});
