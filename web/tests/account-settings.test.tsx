import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { UsersTable } from "@/components/account/admin-users-view";
import { KeysTab } from "@/components/settings/keys-tab";
import { AccessTab } from "@/components/settings/access-tab";
import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
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

describe("OneTimeKeyDialog", () => {
  it("shows the key and does not retain it after close", () => {
    function Host() {
      return <OneTimeKeyDialog secret={state.secret} onClose={() => rerender(null)} />;
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
    expect(screen.queryByText("auk_secret123")).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain("auk_secret123");
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
    wrap(<KeysTab wsId="w" />);
    fireEvent.change(await screen.findByLabelText("Key name"), { target: { value: "ci" } });
    fireEvent.click(screen.getByRole("button", { name: "Create key" }));
    expect(await screen.findByTestId("secret-key")).toHaveTextContent("auk_topsecret");
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
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
    });
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    expect(await screen.findByText("(organization)")).toBeInTheDocument();
    // Remove is in the row's … menu (D6); organization owners have no menu.
    expect(screen.getByRole("button", { name: "More for Bob" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "More for Boss" })).not.toBeInTheDocument();    expect(await screen.findByLabelText("Organization member")).toBeInTheDocument();
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
  const openMenu = (email: string) => fireEvent.keyDown(screen.getByRole("button", { name: `More for ${email}` }), { key: "Enter" });
  const admin = (id: string, over: Partial<AdminUser> = {}) =>
    ({ id, email: `${id}@x.io`, display_name: id, is_instance_admin: false, ...over }) as AdminUser;

  it("your own row offers no Remove admin and no Turn off (P5-POLISH-10)", () => {
    mockFetch({});
    wrap(<UsersTable users={[admin("me", { is_instance_admin: true }), admin("ben", { is_instance_admin: true })]} selfId="me" />);
    expect(screen.getByText("This is you.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "More for me@x.io" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Remove admin|Turn off/ })).toBeNull();
  });

  it("another person's … menu offers Make admin and Turn off, and a change is sent", async () => {
    const calls: string[] = [];
    mockFetch({ "PATCH /v1/admin/users/ben": () => { calls.push("patch"); return json(200, admin("ben", { is_instance_admin: true })); } });
    wrap(<UsersTable users={[admin("me", { is_instance_admin: true }), admin("ben")]} selfId="me" />);
    openMenu("ben@x.io");
    expect(await screen.findByRole("menuitem", { name: "Turn off" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: "Make admin" }));
    await waitFor(() => expect(calls).toEqual(["patch"]));
  });

  it("the only instance admin cannot be removed: Remove admin is not offered, Turn off is", async () => {
    mockFetch({});
    wrap(<UsersTable complete users={[admin("me"), admin("ana", { is_instance_admin: true })]} selfId="me" />);
    openMenu("ana@x.io");
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
    openMenu("ana@x.io");
    expect(await screen.findByRole("menuitem", { name: "Remove admin" })).toBeInTheDocument();
    expect(screen.queryByTestId("idp-role-values")).toBeNull();
    fireEvent.click(screen.getByRole("menuitem", { name: "Show role values (1)" }));
    expect(await screen.findByTestId("idp-role-values")).toBeInTheDocument();
  });
});
