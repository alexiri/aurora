import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ConnectionDetails } from "@/components/accounts/connection-details";
import { wrap } from "./support/render";

vi.mock("next/navigation", () => ({ useParams: () => ({ org: "o1", ws: "w1" }), useSearchParams: () => new URLSearchParams() }));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

// (P5-UX-4) What the old Connections list showed per connection lives under an account row's Technical details.
describe("connection technical details", () => {
  const conn = (over: Record<string, unknown>) => ({
    id: "k1", org_id: "o", workspace_id: "w1", scope: "workspace", app_id: "gmail", package_id: "p", app_version: "1",
    label: "me@acme.io", status: "active", status_reason: null, scopes: ["read"], missing_scopes: [], metadata: {},
    tenant_host: null, oauth_client_id: null, credentials_expires_at: null, connected_by: null, last_used_at: null,
    created_at: "", updated_at: "", injecting: [], ...over,
  });
  async function show(c: { id?: unknown }) {
    vi.stubGlobal("fetch", vi.fn(async () => json(200, { items: [c], next_cursor: null })));
    wrap(<ConnectionDetails scope={{ kind: "workspace", id: "w1" }} id={String(c.id ?? "k1")} canManage />);
    await screen.findByText(/Never used|Last used/);
  }
  it("warns when requested scopes were not granted", async () => {
    await show(conn({ missing_scopes: ["write", "admin"] }));
    expect(screen.getByRole("status")).toHaveTextContent("not granted: write, admin");
  });
  it("shows how to connect to a database account", async () => {
    await show(conn({ id: "0190a1b2-0000-7000-8000-000000000001", app_id: "postgresql", database: { engine: "postgres", service: "orders-prod" } }));
    const note = screen.getByLabelText("How to connect to me@acme.io");
    expect(note).toHaveTextContent("psql service=orders-prod -c 'select 1'");
    expect(note).toHaveTextContent("0190a1b2");
  });
  it("shows when a database account does not verify the server certificate (D-178 Q-R5-7)", async () => {
    await show(conn({ app_id: "postgresql", database: { engine: "postgres", service: "orders" }, metadata: { tls_verification: "off" } }));
    expect(screen.getByLabelText("TLS verification is off for me@acme.io")).toHaveTextContent("does not verify");
  });
  it("shows no TLS warning for a verified database account", async () => {
    await show(conn({ app_id: "postgresql", database: { engine: "postgres", service: "orders" }, metadata: {} }));
    expect(screen.queryByLabelText(/TLS verification is off/)).toBeNull();
  });
  it("shows the scopes a classic token reported and says repo allows writes (FIX-133)", async () => {
    await show(conn({ metadata: { token_scopes: "repo, read:org" } }));
    expect(screen.getByText(/Token scopes/)).toHaveTextContent("repo, read:org");
    expect(screen.getByText(/Token scopes/)).toHaveTextContent("choose Read only");
  });
  it("shows no repo warning on a read-only connection or without a repo scope", async () => {
    await show(conn({ access: "read", metadata: { token_scopes: "repo" } }));
    expect(screen.getByText(/Token scopes/).textContent).not.toMatch(/choose Read only/);
    cleanup();
    await show(conn({ metadata: { token_scopes: "read:user, public_repo" } }));
    expect(screen.getByText(/Token scopes/).textContent).not.toMatch(/choose Read only/);
  });
  it("shows a pasted token's declared scopes and saves an edit (FIX-136)", async () => {
    const methods = [
      {
        id: "token", kind: "fields", fields: [], default_scopes: [], access_levels: [], probe_scopes: ["read:a"],
        scopes: [{ scope: "read:a", name: "Read A", access: "read" }, { scope: "write:a", name: "Write A", access: "write" }],
      },
    ];
    const patched: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (req: Request) => {
        const url = new URL(req.url);
        if (req.method === "PATCH") {
          patched.push(await req.json());
          return json(200, conn({ connect_method: "token", scopes: ["read:a", "write:a"] }));
        }
        if (url.pathname.startsWith("/v1/apps/") && !url.pathname.endsWith("/apps")) return json(200, { app_id: "acme", connect: methods, tools: [] });
        return json(200, { items: [conn({ connect_method: "token", scopes: ["read:a"] })], next_cursor: null });
      }),
    );
    wrap(<ConnectionDetails scope={{ kind: "workspace", id: "w1" }} id="k1" canManage />);
    const box = await screen.findByLabelText("Token scopes of me@acme.io");
    expect(box).toHaveTextContent("as you declared");
    expect(box).toHaveTextContent("Read A");
    fireEvent.click(screen.getByRole("button", { name: "Edit scopes" }));
    fireEvent.click(within(await screen.findByRole("radiogroup", { name: "Write A" })).getByLabelText("On"));
    fireEvent.click(screen.getByRole("button", { name: "Save scopes" }));
    await waitFor(() => expect(patched).toEqual([{ scopes: ["read:a", "write:a"] }]));
  });
  it("offers no scope editing to people who cannot manage the account", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => json(200, { items: [conn({ connect_method: "token" })], next_cursor: null })));
    wrap(<ConnectionDetails scope={{ kind: "workspace", id: "w1" }} id="k1" canManage={false} />);
    await screen.findByText(/Never used/);
    expect(screen.queryByRole("button", { name: "Edit scopes" })).toBeNull();
  });
  it("shows no connection help for other apps", async () => {
    await show(conn({ database: null }));
    expect(screen.queryByLabelText("How to connect to me@acme.io")).toBeNull();
  });
  it("explains a plugin major upgrade flip without the internal detail (RFE-022)", async () => {
    await show(conn({ status: "reauth_required", status_reason: "plugin_major_upgrade: p 1.0.0 -> 2.0.0; missing: token_v2" }));
    expect(screen.getByText(/gmail was upgraded to a version with different credential fields/)).toBeInTheDocument();
    expect(screen.queryByText(/missing: token_v2/)).toBeNull();
  });
  it("names the instance and says a deleted instance means connecting again", async () => {
    await show(
      conn({ instance: { id: "i1", host: "desk.acme.example", base_path: "/jira", base_url: "https://desk.acme.example/jira", label: "Acme desk" } }),
    );
    expect(screen.getByText(/https:\/\/desk\.acme\.example\/jira/)).toBeInTheDocument();
    cleanup();
    await show(conn({ status: "disabled", status_reason: "instance_deleted" }));
    expect(screen.getByText(/self-hosted app of this connection was deleted/)).toBeInTheDocument();
  });
  it("shows no warning for a full grant", async () => {
    await show(conn({}));
    expect(screen.queryByRole("status")).toBeNull();
  });
});
