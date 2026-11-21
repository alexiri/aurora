import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { UsersTable } from "@/components/account/admin-users-view";
import { KeysTab } from "@/components/settings/keys-tab";
import { AccessTab } from "@/components/settings/access-tab";
import { SecretOnce } from "@/components/ui/secret-once";
import type { AdminUser } from "@/lib/api/account";
import { wrap } from "./support/render";
import { json } from "./support/api";

function mockFetch(routes: Record<string, () => Response>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const h = routes[`${req?.method ?? "GET"} ${url.pathname}`];
      return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

describe("SecretOnce", () => {
  it("shows the key, asks before closing without a copy, and does not retain it after close", () => {
    function Host() {
      return <SecretOnce secret={state.secret} title="Copy your new API key" description="You won't see it again, so store it somewhere safe now." onClose={() => rerender(null)} />;
    }
    const state = { secret: "auk_secret123" as string | null };
    const { rerender: r } = render(<Host />);
    const rerender = (s: string | null) => {
      state.secret = s;
      r(<Host />);
    };
    expect(screen.getByTestId("secret-key")).toHaveTextContent("auk_secret123");
    expect(screen.getByText(/won't see it again/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    // Never copied: the dialog asks first and keeps the secret.
    expect(screen.getByText("You haven't copied it. Close anyway?")).toBeInTheDocument();
    expect(screen.getByTestId("secret-key")).toHaveTextContent("auk_secret123");
    fireEvent.click(screen.getByRole("button", { name: "Close anyway" }));
    expect(screen.queryByText("auk_secret123")).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain("auk_secret123");
  });
});

describe("KeysTab rows and revoke (workspace-6)", () => {
  it("says what a key can do and when it was used, and asks before revoking", async () => {
    const key = (id: string, name: string, last: string | null) => ({
      id, name, kind: "service", org_id: "o", workspace_id: "w", display_prefix: "auk_ab", owner: { id: "u", display_name: "U" },
      created_at: "2026-01-01T00:00:00Z", last_used_at: last,
    });
    const revoked: string[] = [];
    mockFetch({
      "GET /v1/workspaces/w/api-keys": () => json(200, { items: [key("k1", "ci-script", "2026-11-20T10:00:00Z"), key("k2", "notebook", null)], next_cursor: null }),
      "DELETE /v1/api-keys/k1": () => {
        revoked.push("k1");
        return json(204, null);
      },
    });
    wrap(<KeysTab wsId="w" workspaceName="Bob's Lab" />);
    const list = await screen.findByRole("list", { name: "API keys" });
    expect(list).toHaveTextContent("ci-script");
    expect(list).toHaveTextContent("Can do everything you can in Bob's Lab");
    expect(list).toHaveTextContent("never used");
    fireEvent.click(screen.getByRole("button", { name: "Revoke ci-script" }));
    expect(await screen.findByText("Revoke ci-script?")).toBeInTheDocument();
    expect(screen.getByText("Anything using it stops working now.")).toBeInTheDocument();
    expect(revoked).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Revoke key" }));
    await waitFor(() => expect(revoked).toEqual(["k1"]));
  });
});

describe("KeysTab", () => {
  it("reveals the created key once and drops it on close", async () => {
    const apiKey = {
      id: "k1", name: "ci", kind: "service", org_id: "o", workspace_id: "w", display_prefix: "auk_ab",
      owner: { id: "u", display_name: "U" }, created_at: "2026-01-01T00:00:00Z",
    };
    mockFetch({
      "GET /v1/workspaces/w/api-keys": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/workspaces/w/api-keys": () => json(201, { api_key: apiKey, key: "auk_topsecret" }),
    });
    wrap(<KeysTab wsId="w" workspaceName="Bob's Lab" />);
    expect(await screen.findByText(/Acts as you in this workspace through the Aurora API/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "OpenAPI description" })).toHaveAttribute("href", "/v1/openapi.json");
    fireEvent.click(await screen.findByRole("button", { name: "New API key" }));
    fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "ci" } });
    fireEvent.click(screen.getByRole("button", { name: "Create key" }));
    expect(await screen.findByTestId("secret-key")).toHaveTextContent("auk_topsecret");
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    fireEvent.click(screen.getByRole("button", { name: "Close anyway" }));
    await waitFor(() => expect(document.body.textContent).not.toContain("auk_topsecret"));
  });
});

const bindings = {
  items: [
    { user_id: "a", email: "boss@x.io", display_name: "Boss", role: "owner", implicit: true },
    { user_id: "b", email: "bob@x.io", display_name: "Bob", role: "member", implicit: false },
  ],
  next_cursor: null,
};
const members = {
  items: [{ user_id: "c", email: "cy@x.io", display_name: "Cy", role: "member", status: "active", created_at: "x" }],
  next_cursor: null,
};

describe("AccessTab", () => {
  it("owner sees edit controls, implicit badge and member picker", async () => {
    mockFetch({
      "GET /v1/workspaces/w/access": () => json(200, bindings),
      "GET /v1/orgs/o/members": () => json(200, members),
      "GET /v1/orgs/o": () => json(200, { id: "o", slug: "users", name: "Users", my_role: "admin" }),
    });
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    // (D6) The inherited owner's reason is a line under the person.
    expect(await screen.findByText("Has access as an owner of Users")).toBeInTheDocument();
    // Remove is in the row's … menu (D6); organization owners have no menu.
    expect(screen.getByRole("button", { name: "More for Bob" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "More for Boss" })).not.toBeInTheDocument();    expect(await screen.findByLabelText("Organization member")).toBeInTheDocument();
    // (workspace-7) The role choices are words, not the raw role values.
    const role = screen.getByLabelText("Role") as HTMLSelectElement;
    expect(Array.from(role.options).map((o) => o.textContent)).toEqual(["Member", "Approver", "Owner"]);
    expect(role.selectedOptions[0]!.textContent).toBe("Member");
  });
  it("member sees a read-only list", async () => {
    mockFetch({ "GET /v1/workspaces/w/access": () => json(200, bindings) });
    wrap(<AccessTab wsId="w" orgId="o" canOwn={false} />);
    expect(await screen.findByText("Bob")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Remove/ })).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Organization member")).not.toBeInTheDocument();
    expect(screen.getByText(/Only workspace owners/)).toBeInTheDocument();
  });
  it("hides the picker when listing members is forbidden", async () => {
    mockFetch({
      "GET /v1/workspaces/w/access": () => json(200, bindings),
      "GET /v1/orgs/o/members": () => json(403, { error: { code: "forbidden", message: "no" } }),
    });
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    expect(await screen.findByText("Bob")).toBeInTheDocument();
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByLabelText("Organization member")).not.toBeInTheDocument();
  });
});

describe("UsersTable", () => {
  const openMenu = (name: string) => fireEvent.keyDown(screen.getByRole("button", { name: `Manage ${name}` }), { key: "Enter" });
  const admin = (id: string, over: Partial<AdminUser> = {}) =>
    ({ id, email: `${id}@x.io`, display_name: id, is_instance_admin: false, ...over }) as AdminUser;

  it("(batch 8, b7#8c) a person stored by their address is named Erin, and the address is shown once, as the address", () => {
    mockFetch({});
    wrap(<UsersTable users={[admin("erin", { display_name: "erin@x.io" })]} selfId="me" />);
    expect(screen.getByText("Erin")).toBeInTheDocument();
    expect(screen.getByText("erin@x.io · User")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Manage Erin" })).toBeInTheDocument();
  });

  it("shows the display name, the e-mail's name and the role in words, a Manage button and 'Roles from sign-in: none' (admin-13)", () => {
    mockFetch({});
    wrap(<UsersTable users={[admin("alice", { display_name: "Alice Admin", is_instance_admin: true }), admin("bob", { display_name: "Bob Builder" })]} selfId="me" />);
    expect(screen.getByText("Alice Admin")).toBeInTheDocument();
    expect(screen.getByText("alice · Instance admin")).toBeInTheDocument();
    expect(screen.getByText("bob · User")).toBeInTheDocument();
    const manage = screen.getByRole("button", { name: "Manage Bob Builder" });
    expect(manage).toHaveTextContent("Manage");
    expect(screen.getByText("Roles from sign-in: none")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^More for / })).toBeNull();
  });

  it("your own row offers no Remove admin and no Turn off (P5-POLISH-10)", () => {
    mockFetch({});
    wrap(<UsersTable users={[admin("me", { is_instance_admin: true }), admin("ben", { is_instance_admin: true })]} selfId="me" />);
    expect(screen.getByText("This is you.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Manage me" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Remove admin|Turn off/ })).toBeNull();
  });

  it("another person's … menu offers Make admin and Turn off, and a change is sent", async () => {
    const calls: string[] = [];
    mockFetch({ "PATCH /v1/admin/users/ben": () => { calls.push("patch"); return json(200, admin("ben", { is_instance_admin: true })); } });
    wrap(<UsersTable users={[admin("me", { is_instance_admin: true }), admin("ben")]} selfId="me" />);
    openMenu("ben");
    expect(await screen.findByRole("menuitem", { name: "Turn off" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: "Make admin" }));
    await waitFor(() => expect(calls).toEqual(["patch"]));
  });

  it("the only instance admin cannot be removed: Remove admin is not offered, Turn off is", async () => {
    mockFetch({});
    wrap(<UsersTable complete users={[admin("me"), admin("ana", { is_instance_admin: true })]} selfId="me" />);
    openMenu("ana");
    expect(await screen.findByRole("menuitem", { name: "Turn off" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Remove admin" })).toBeNull();
  });

  it("with another admin in the list, Remove admin is offered; role values open from the menu", async () => {
    mockFetch({});
    wrap(
      <UsersTable
        complete
        users={[admin("me"), admin("ana", { is_instance_admin: true, idp_roles: { values: ["ops"], granted: [], provider: "corp", synced_at: "2026-11-19T10:00:00Z" } }), admin("bo", { is_instance_admin: true })]}
        selfId="me"
      />,
    );
    openMenu("ana");
    expect(await screen.findByRole("menuitem", { name: "Remove admin" })).toBeInTheDocument();
    expect(screen.queryByTestId("idp-role-values")).toBeNull();
    fireEvent.click(screen.getByRole("menuitem", { name: "Show role values (1)" }));
    expect(await screen.findByTestId("idp-role-values")).toBeInTheDocument();
  });
});
