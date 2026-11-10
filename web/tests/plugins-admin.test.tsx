import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ConflictGroup } from "@/components/admin/host-claims-view";
import { PluginReview } from "@/components/admin/plugin-detail-view";
import { UploadForm } from "@/components/admin/plugins-view";
import type { HostClaimConflict, PluginPackage } from "@/lib/api/plugins";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

interface Call {
  method: string;
  path: string;
  search: string;
  body: string | FormData | null;
}

function mockFetch(handler: (c: Call) => Response) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      let body: string | FormData | null = (init?.body as string | FormData | null | undefined) ?? null;
      if (req && req.method !== "GET" && req.method !== "DELETE") {
        const ct = req.headers.get("content-type") ?? "";
        body = ct.startsWith("multipart/") ? await req.clone().formData() : await req.clone().text();
      }
      const call: Call = {
        method: req?.method ?? init?.method ?? "GET",
        path: url.pathname,
        search: url.search,
        body,
      };
      calls.push(call);
      return handler(call);
    }),
  );
  return calls;
}

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const plugin: PluginPackage = {
  id: "p1",
  package_id: "acme.git",
  version: "1.0.0",
  contract: 1,
  publisher: "Acme",
  digest: "abc",
  signer: { kind: "trusted_key", key_id: "k1", name: "Acme key" },
  source: "upload",
  status: "pending_approval",
  status_reason: null,
  violations: [],
  apps: [{ app_id: "git", name: "<b>Git</b>" }],
  permissions: { hooks: ["token.mint"], egress: { "token.mint": ["api.acme.test"] }, context: ["org.name"] },
  host_claims: [],
  widenings: [{ kind: "egress", detail: "adds api.acme.test" }],
  impact: null,
  approved_by: null,
  approved_at: null,
  created_at: "2026-01-01T00:00:00Z",
};

describe("PluginReview", () => {
  it("disables approve until every section is expanded", async () => {
    const calls = mockFetch(() => json(200, plugin));
    wrap(<PluginReview plugin={plugin} />);
    const approve = screen.getByRole("button", { name: "Approve" });
    expect(approve).toBeDisabled();
    for (const name of [/Hooks/, /Egress per hook/, /Context variables/]) {
      fireEvent.click(screen.getByRole("button", { name }));
    }
    expect(approve).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /Host claims/ }));
    expect(approve).toBeEnabled();
    expect(screen.getByText(/adds api.acme.test/)).toBeInTheDocument();
    // plugin text is plain text, not markup
    expect(screen.getByText("<b>Git</b>", { exact: false })).toBeInTheDocument();
    fireEvent.click(approve);
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/admin/plugins/p1/approve")).toBe(true));
  });

  it("shows the impact banner and gates approval on the Impact section of a major upgrade", async () => {
    const major: PluginPackage = {
      ...plugin,
      version: "2.0.0",
      status_reason: "breaks_connections",
      impact: {
        major_upgrade: true,
        from_version: "1.0.0",
        connections_total: 5,
        connections_needing_reauth: 3,
        by_app: [{ app_id: "git", connections: 5, needing_reauth: 3 }],
      },
    };
    const calls = mockFetch(() => json(200, { ...major, status: "active", connections_reauth: 3 }));
    wrap(<PluginReview plugin={major} />);
    expect(screen.getByRole("status", { name: "Upgrade impact" })).toHaveTextContent("3 connections will need reconnecting");
    const approve = screen.getByRole("button", { name: "Approve" });
    for (const name of [/Hooks/, /Egress per hook/, /Context variables/, /Host claims/]) {
      fireEvent.click(screen.getByRole("button", { name }));
    }
    // The four permission sections are not enough: the Impact section is mandatory too.
    expect(approve).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /Impact on connections/ }));
    expect(approve).toBeEnabled();
    expect(screen.getByText(/3 of 5 to reconnect/)).toBeInTheDocument();
    fireEvent.click(approve);
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/admin/plugins/p1/approve")).toBe(true));
    expect(await screen.findByText(/Approved\. 3 connections will need reconnecting/)).toBeInTheDocument();
  });

  it("shows the grant conversion banner and gates approval on its section (P4-GRANTS)", async () => {
    const conv: PluginPackage = {
      ...plugin,
      version: "1.1.0",
      status_reason: "converts_grants",
      impact: {
        major_upgrade: false,
        from_version: "1.0.0",
        connections_total: 2,
        connections_needing_reauth: 0,
        grants_to_convert: 3,
        by_app: [{ app_id: "indico", connections: 2, needing_reauth: 0, grants_to_convert: 3, grant_workspaces: 2 }],
      },
    };
    const calls = mockFetch(() => json(200, { ...conv, status: "active", connections_reauth: 0 }));
    wrap(<PluginReview plugin={conv} />);
    expect(screen.getByRole("status", { name: "Grant conversion" })).toHaveTextContent(
      "3 agent grants will be converted from full to custom access",
    );
    // Not a major upgrade: no reconnect banner.
    expect(screen.queryByRole("status", { name: "Upgrade impact" })).toBeNull();
    const approve = screen.getByRole("button", { name: "Approve" });
    for (const name of [/Hooks/, /Egress per hook/, /Context variables/, /Host claims/]) {
      fireEvent.click(screen.getByRole("button", { name }));
    }
    expect(approve).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /Grants to convert/ }));
    expect(approve).toBeEnabled();
    expect(screen.getByText(/indico/)).toBeInTheDocument();
    expect(screen.getByText(/3 grants in/)).toBeInTheDocument();
    fireEvent.click(approve);
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/admin/plugins/p1/approve")).toBe(true));
  });

  it("shows no impact banner or section for a non-major version", () => {
    mockFetch(() => json(200, plugin));
    wrap(
      <PluginReview
        plugin={{
          ...plugin,
          impact: { major_upgrade: false, from_version: "1.0.0", connections_total: 2, connections_needing_reauth: 0, by_app: [] },
        }}
      />,
    );
    expect(screen.queryByRole("status", { name: "Upgrade impact" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Impact on connections/ })).toBeNull();
  });

  it("asks for force confirmation on live_connections", async () => {
    const calls = mockFetch((c) =>
      c.method === "DELETE" && !c.search
        ? json(409, { error: { code: "live_connections", message: "x", details: { count: 3 } } })
        : c.method === "DELETE"
          ? json(204, null)
          : json(200, plugin),
    );
    wrap(<PluginReview plugin={plugin} />);
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    expect(await screen.findByText(/3 connection\(s\)/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Uninstall anyway" }));
    await waitFor(() =>
      expect(calls.some((c) => c.method === "DELETE" && c.search === "?force=true")).toBe(true),
    );
    expect(await screen.findByText(/Plugin uninstalled/)).toBeInTheDocument();
  });
});

describe("UploadForm", () => {
  it("renders violations from a 422", async () => {
    const calls = mockFetch(() =>
      json(422, {
        error: {
          code: "package_invalid",
          message: "invalid",
          details: { violations: [{ code: "bad_egress", path: "/hooks/0", message: "host not allowed" }] },
        },
      }),
    );
    wrap(<UploadForm />);
    const file = new File(["x"], "p.tar.gz");
    fireEvent.change(screen.getByLabelText("Plugin package"), { target: { files: [file] } });
    fireEvent.click(screen.getByRole("button", { name: "Upload" }));
    expect(await screen.findByText("bad_egress")).toBeInTheDocument();
    expect(screen.getByText(/host not allowed/)).toBeInTheDocument();
    expect((calls[0].body as FormData).get("package")).toBeTruthy();
  });
});

describe("ConflictGroup", () => {
  it("PUTs the group key and fingerprint", async () => {
    const c: HostClaimConflict = {
      group_key: "api.example.com/",
      fingerprint: "fp1",
      claims: [
        { package_id: "a.pkg", app_id: "a", claim_key: "k1" },
        { package_id: "b.pkg", app_id: "b", claim_key: "k2" },
      ],
      decision: null,
    };
    const calls = mockFetch(() => json(200, c));
    wrap(<ConflictGroup c={c} />);
    fireEvent.click(screen.getByRole("button", { name: /Pick b.pkg \/ b/ }));
    await waitFor(() => expect(calls.length).toBeGreaterThan(0));
    expect(calls[0].method).toBe("PUT");
    expect(calls[0].path).toBe("/v1/admin/host-claims/api.example.com%2F");
    expect(JSON.parse(calls[0].body as string)).toEqual({
      winner_package_id: "b.pkg",
      winner_app_id: "b",
      fingerprint: "fp1",
    });
  });
});
