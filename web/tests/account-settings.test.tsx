import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { UsersTable } from "@/components/account/admin-users-view";
import { KeysTab } from "@/components/settings/keys-tab";
import { AccessTab } from "@/components/settings/access-tab";
import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import type { AdminUser } from "@/lib/api/account";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

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

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

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
    expect(await screen.findByText("Implicit (org)")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove bob@x.io" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove boss@x.io" })).not.toBeInTheDocument();
    expect(await screen.findByLabelText("Organization member")).toBeInTheDocument();
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
  it("explains cannot_modify_self", async () => {
    mockFetch({
      "PATCH /v1/admin/users/me": () =>
        json(422, { error: { code: "cannot_modify_self", message: "x" } }),
    });
    const u = { id: "me", email: "me@x.io", display_name: "Me", is_instance_admin: true } as AdminUser;
    wrap(<UsersTable users={[u]} selfId="me" />);
    fireEvent.click(screen.getByRole("button", { name: "Remove admin from me@x.io" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/cannot change your own/);
  });
});
