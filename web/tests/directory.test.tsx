import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { OrgDirectoryView } from "@/components/org/directory-view";
import { MemberRow } from "@/components/org/members-view";

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

const ORG = "00000000-0000-4000-8000-000000000001";
const now = "2026-11-10T10:00:00Z";

const settings = (over: Record<string, unknown> = {}) => ({
  available: true,
  enabled: true,
  email_domains: ["example.com"],
  exclusive: false,
  base_url: "https://aurora.example.test/scim/v2",
  ...over,
});

const tokens = [
  {
    id: "t1",
    name: "Entra ID",
    display_prefix: "aud_abcdef0123",
    created_at: now,
    expires_at: "2027-11-10T10:00:00Z",
    last_used_at: null,
    revoked: false,
    expiring: false,
  },
  {
    id: "t2",
    name: "old",
    display_prefix: "aud_111111111111",
    created_at: now,
    expires_at: "2026-11-20T10:00:00Z",
    last_used_at: now,
    revoked: false,
    expiring: true,
  },
];

const status = {
  enabled: true,
  last_request_at: "2026-11-10T09:59:00Z",
  users: { active: 2, suspended: 1, removed: 3 },
  groups: 2,
  errors: [
    { at: now, method: "POST", resource: "/Users", status: 400, scim_type: "invalidValue" },
  ],
};

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

/** Routes by `METHOD path`; records every call as `METHOD path body`. */
function stub(routes: Record<string, (body: unknown) => Response>) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const method = req?.method ?? init?.method ?? "GET";
      const raw = req ? (method === "GET" ? null : await req.text().catch(() => null)) : (init?.body as string | undefined);
      const body = raw ? (JSON.parse(raw) as unknown) : undefined;
      calls.push(
        `${method} ${url.pathname}${body === undefined ? "" : ` ${JSON.stringify(body)}`}`,
      );
      const h = routes[`${method} ${url.pathname}`];
      return h ? h(body) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("Directory sync page", () => {
  it("hides everything when the instance switch is off", async () => {
    stub({ [`GET /v1/orgs/${ORG}/scim`]: () => json(200, settings({ available: false })) });
    wrap(<OrgDirectoryView org={{ id: ORG, slug: "acme" }} isOwner />);
    expect(
      await screen.findByText(/not enabled on this Aurora instance/),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText(/Allowed e-mail domains/)).not.toBeInTheDocument();
  });

  it("shows the base URL, settings and status for an owner", async () => {
    stub({
      [`GET /v1/orgs/${ORG}/scim`]: () => json(200, settings()),
      [`GET /v1/orgs/${ORG}/scim/tokens`]: () => json(200, { tokens }),
      [`GET /v1/orgs/${ORG}/scim/status`]: () => json(200, status),
      "GET /v1/instance": () => json(200, { oidc_providers: [] }),
    });
    wrap(<OrgDirectoryView org={{ id: ORG, slug: "acme" }} isOwner />);
    expect(await screen.findByDisplayValue("https://aurora.example.test/scim/v2")).toBeInTheDocument();
    expect(screen.getByLabelText(/Allowed e-mail domains/)).toHaveValue("example.com");
    expect(await screen.findByText("Entra ID")).toBeInTheDocument();
    expect(screen.getByText(/2 active, 1 suspended, 3 removed/)).toBeInTheDocument();
    expect(screen.getByText(/Last request/i)).toBeInTheDocument();
    expect(screen.getByText(/invalidValue/)).toBeInTheDocument();
    // The expiring token carries the banner badge.
    expect(screen.getByText("expiring")).toBeInTheDocument();
  });

  it("a non-owner sees the note and no settings", async () => {
    stub({ [`GET /v1/orgs/${ORG}/scim`]: () => json(200, settings()) });
    wrap(<OrgDirectoryView org={{ id: ORG, slug: "acme" }} isOwner={false} />);
    expect(await screen.findByText(/Only organization owners/)).toBeInTheDocument();
    expect(screen.queryByLabelText(/Allowed e-mail domains/)).not.toBeInTheDocument();
  });

  it("saves settings and shows the re-auth hint on 412", async () => {
    let n = 0;
    const calls = stub({
      [`GET /v1/orgs/${ORG}/scim`]: () =>
        json(200, n++ === 0 ? settings() : settings({ exclusive: true })),
      [`PUT /v1/orgs/${ORG}/scim`]: (body) =>
        (body as { exclusive?: boolean }).exclusive
          ? json(412, { error: { code: "reauth_required", message: "reauth" } })
          : json(200, settings()),
      "GET /v1/instance": () => json(200, { oidc_providers: [] }),
    });
    wrap(<OrgDirectoryView org={{ id: ORG, slug: "acme" }} isOwner />);
    const exclusive = await screen.findByRole("checkbox", { name: /Directory-managed membership/ });
    fireEvent.click(exclusive);
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(/Confirm it is you/),
    );
    expect(
      calls.some((c) => c.startsWith(`PUT /v1/orgs/${ORG}/scim`)),
    ).toBe(true);
  });

  it("creates a token, shows it once, then only the prefix", async () => {
    const calls = stub({
      [`GET /v1/orgs/${ORG}/scim`]: () => json(200, settings()),
      [`GET /v1/orgs/${ORG}/scim/tokens`]: () => json(200, { tokens }),
      [`POST /v1/orgs/${ORG}/scim/tokens`]: () =>
        json(201, { ...tokens[0], id: "t3", name: "Okta", token: "aud_NEW1234567890abcdef" }),
    });
    wrap(<OrgDirectoryView org={{ id: ORG, slug: "acme" }} isOwner />);
    fireEvent.change(await screen.findByLabelText("New token name"), {
      target: { value: "Okta" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create token" }));
    expect(await screen.findByText(/shown only once/)).toBeInTheDocument();
    expect(screen.getByText("aud_NEW1234567890abcdef")).toBeInTheDocument();
    // Never sent again: the list call still returns only the prefixes.
    expect(calls.some((c) => c.includes("aud_NEW1234567890abcdef") && c.startsWith("GET"))).toBe(
      false,
    );
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(screen.getByText("Copied")).toBeInTheDocument();
  });

  it("revokes a token", async () => {
    const calls = stub({
      [`GET /v1/orgs/${ORG}/scim`]: () => json(200, settings()),
      [`GET /v1/orgs/${ORG}/scim/tokens`]: () => json(200, { tokens }),
      [`DELETE /v1/scim/tokens/t1`]: () => new Response(null, { status: 204 }),
    });
    wrap(<OrgDirectoryView org={{ id: ORG, slug: "acme" }} isOwner />);
    expect(await screen.findByText("Entra ID")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: "Revoke" })[0]!);
    await waitFor(() =>
      expect(calls.some((c) => c.startsWith("DELETE /v1/scim/tokens/t1"))).toBe(true),
    );
  });
});

describe("Members Directory badge", () => {
  const m = (directory: { state: "active" | "suspended" } | null) => ({
    user_id: "u1",
    email: "scim@example.com",
    display_name: "S",
    role: "member" as const,
    status: "active" as const,
    created_at: "2026-01-01T00:00:00Z",
    directory,
  });
  const noop = () => {};

  it("shows the Directory badge for a provisioned member", () => {
    render(
      <table><tbody><MemberRow
        m={m({ state: "active" })}
        viewer="owner"
        onRole={noop}
        onStatus={noop}
        onRemove={noop}
      /></tbody></table>,
    );
    expect(screen.getByText(/Directory/)).toBeInTheDocument();
  });

  it("marks a suspended link", () => {
    render(
      <table><tbody><MemberRow
        m={m({ state: "suspended" })}
        viewer="owner"
        onRole={noop}
        onStatus={noop}
        onRemove={noop}
      /></tbody></table>,
    );
    expect(screen.getByText("Directory (suspended)")).toBeInTheDocument();
  });

  it("no badge without a link", () => {
    render(
      <table><tbody><MemberRow
        m={m(null)}
        viewer="owner"
        onRole={noop}
        onStatus={noop}
        onRemove={noop}
      /></tbody></table>,
    );
    expect(screen.queryByText(/Directory/)).not.toBeInTheDocument();
  });
});
