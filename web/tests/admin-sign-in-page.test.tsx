import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/people",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams("tab=sign-in"),
  useParams: () => ({}),
}));

import { AdminPeoplePage } from "@/components/admin/admin-topics";

const schema = (kind: string, extra: Record<string, unknown> = {}) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null, ...extra });
const base = { locked: false, helm_key: "config.x", list: null, id_field: null, entries: null, secret: null, default: null };

const cern = {
  id: "cern",
  display_name: "CERN SSO",
  issuer: "https://auth.example.test/realms/cern",
  client_id: "c",
  role_claim: "cern_roles",
  role_mappings: [
    { value: "aurora-admins", instance_admin: true },
    { value: "aurora-users", instance_admin: false, org: { slug: "users", role: "member" } },
  ],
};

const SECTIONS: Record<string, unknown> = {
  sign_in: {
    section: "sign_in",
    title: "Sign-in",
    description: "Sign-up, sessions and identity providers.",
    generation: 1,
    updated_at: null,
    updated_by_label: null,
    extra: { signed_in_with: "cern" },
    settings: [
      { ...base, key: "auth.signup_mode", value: "invite_only", source: "default", default: "invite_only", schema: schema("enum", { options: ["invite_only", "sso_jit", "open"] }) },
      {
        ...base,
        key: "auth.oidc_providers",
        value: [cern],
        source: "helm",
        default: [],
        list: "keyed",
        id_field: "id",
        schema: schema("list", { item_kind: "object" }),
        entries: [{ id: "cern", value: cern, source: "helm", locked: true, warnings: null }],
      },
    ],
  },
  scim: {
    section: "scim",
    title: "Directory sync",
    description: "SCIM.",
    generation: 1,
    updated_at: null,
    updated_by_label: null,
    extra: { available: true },
    settings: [{ ...base, key: "scim.enabled", value: false, source: "default", default: false, schema: schema("bool") }],
  },
};

function mock(me: Record<string, unknown>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = typeof input === "string" ? new Request(new URL(input, "http://localhost")) : (input as Request);
      const url = new URL(req.url, "http://localhost");
      if (req.method === "GET" && url.pathname === "/v1/me") return json(200, me);
      const m = /^\/v1\/admin\/settings\/(\w+)$/.exec(url.pathname);
      if (req.method === "GET" && m && SECTIONS[m[1]!]) return json(200, SECTIONS[m[1]!]);
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

describe("Administration › People › Sign-in (P5-IA-4, C-968)", () => {
  it("is one page with three headed parts: providers, roles from the provider, directory sync", async () => {
    mock({
      user: { id: "u", is_instance_admin: true, display_name: "Root", email: "r@x.io" },
      idp_roles: {
        provider: "cern",
        synced_at: "2026-11-15T10:00:00Z",
        values: ["aurora-admins", "aurora-users"],
        granted: [
          { value: "aurora-admins", instance_admin: true, workspaces: [] },
          { value: "aurora-users", instance_admin: false, org: { slug: "users", role: "member" }, workspaces: [] },
        ],
      },
    });
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <AdminPeoplePage />
      </QueryClientProvider>,
    );
    const roles = await screen.findByRole("region", { name: "Roles from the provider" });
    expect(screen.getByRole("heading", { name: "How people sign in" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Directory sync" })).toBeInTheDocument();
    // The Identity page's mapping view: the claim, each value and what it gives.
    expect(await within(roles).findByText("cern_roles")).toBeInTheDocument();
    const table = within(roles).getByRole("table", { name: "Roles from CERN SSO" });
    expect(within(table).getByText("aurora-admins").closest("tr")).toHaveTextContent("Aurora administrator");
    expect(within(table).getByText("aurora-users").closest("tr")).toHaveTextContent("member in organization users");
    expect(await within(roles).findByTestId("my-roles")).toHaveTextContent("You signed in through CERN SSO");
    // The providers and the directory sync form are on the same page, and there is no separate Identity page.
    expect(await screen.findByRole("list", { name: "Identity providers" })).toBeInTheDocument();
    await vi.waitFor(() => expect(document.getElementById("scim.enabled-c")).not.toBeNull());
    expect(screen.queryByRole("heading", { name: "Identity" })).toBeNull();
  });
});
