import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { InviteForm } from "@/components/org/invite-dialog";
import { MemberRow } from "@/components/org/members-view";
import { OrgDangerZone } from "@/components/org/settings-view";
import { canManageMember, roleOptions, validateInvite } from "@/lib/api/org";

vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: vi.fn() }) }));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

function mockFetch(routes: Record<string, () => Response>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : null;
    const url = new URL(req ? req.url : String(input), "http://localhost");
    const h = routes[`${req?.method ?? "GET"} ${url.pathname}`];
    return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("role-change options", () => {
  it("owner viewer may assign any role", () => {
    expect(roleOptions("owner", "member")).toEqual(["owner", "admin", "member"]);
    expect(roleOptions("owner", "owner")).toEqual(["owner", "admin", "member"]);
  });
  it("admin viewer cannot grant owner or touch owners", () => {
    expect(roleOptions("admin", "member")).toEqual(["admin", "member"]);
    expect(roleOptions("admin", "owner")).toEqual([]);
    expect(canManageMember("admin", "owner")).toBe(false);
    expect(canManageMember("admin", "admin")).toBe(true);
  });
  it("members get nothing", () => {
    expect(roleOptions("member", "member")).toEqual([]);
    expect(canManageMember("member", "member")).toBe(false);
  });

  const m = (role: "owner" | "admin" | "member") => ({
    user_id: "u1",
    email: "x@example.com",
    display_name: "X",
    role,
    status: "active" as const,
    created_at: "2026-01-01T00:00:00Z",
  });
  const noop = () => {};

  it("renders owner option only for owner viewer", () => {
    const { unmount } = render(
      <table><tbody><MemberRow m={m("member")} viewer="owner" onRole={noop} onStatus={noop} onRemove={noop} /></tbody></table>,
    );
    expect(screen.getByRole("option", { name: "owner" })).toBeInTheDocument();
    unmount();
    render(
      <table><tbody><MemberRow m={m("member")} viewer="admin" onRole={noop} onStatus={noop} onRemove={noop} /></tbody></table>,
    );
    expect(screen.queryByRole("option", { name: "owner" })).not.toBeInTheDocument();
  });

  it("admin sees no controls on an owner row", () => {
    render(
      <table><tbody><MemberRow m={m("owner")} viewer="admin" onRole={noop} onStatus={noop} onRemove={noop} /></tbody></table>,
    );
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Remove/ })).not.toBeInTheDocument();
  });

  it("a pending member (D-175) shows as waiting for first sign-in and cannot be activated", () => {
    render(
      <table>
        <tbody>
          <MemberRow
            m={{ ...m("member"), status: "pending" as const }}
            viewer="owner"
            onRole={noop}
            onStatus={noop}
            onRemove={noop}
          />
        </tbody>
      </table>,
    );
    expect(screen.getByText("Waiting for first sign-in")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Reactivate|Suspend/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Remove/ })).toBeInTheDocument();
  });
});

describe("invite form", () => {
  it("validates email", () => {
    expect(validateInvite({ email: "nope", role: "member" })).toMatch(/valid email/);
    expect(validateInvite({ email: "a@b.co", role: "admin" })).toBeNull();
  });

  it("blocks invalid email without calling the API", () => {
    const fetchFn = mockFetch({});
    wrap(<InviteForm orgId="o1" onDone={() => {}} />);
    fireEvent.change(screen.getByLabelText("Email"), { target: { value: "bad" } });
    fireEvent.click(screen.getByRole("button", { name: "Send invitation" }));
    expect(screen.getByRole("alert")).toHaveTextContent(/valid email/);
    expect(fetchFn.mock.calls.some(([r]) => (r as Request).method === "POST")).toBe(false);
  });

  it("shows the seat cap error", async () => {
    mockFetch({
      "GET /v1/orgs/o1/workspaces": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/orgs/o1/invitations": () =>
        json(422, { error: { code: "member_limit_reached", message: "full" } }),
    });
    const onDone = vi.fn();
    wrap(<InviteForm orgId="o1" onDone={onDone} />);
    fireEvent.change(screen.getByLabelText("Email"), { target: { value: "a@b.co" } });
    fireEvent.click(screen.getByRole("button", { name: "Send invitation" }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(/member limit/i),
    );
    expect(onDone).not.toHaveBeenCalled();
  });
});

describe("org danger zone", () => {
  it("only enables delete when the slug matches", async () => {
    const fetchFn = mockFetch({ "DELETE /v1/orgs/o1": () => json(204, null) });
    const onDeleted = vi.fn();
    render(<OrgDangerZone orgId="o1" slug="acme" onDeleted={onDeleted} />);
    const btn = screen.getByRole("button", { name: "Delete organization" });
    expect(btn).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Organization URL name"), { target: { value: "acm" } });
    expect(btn).toBeDisabled();
    expect(fetchFn).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Organization URL name"), { target: { value: "acme" } });
    expect(btn).toBeEnabled();
    fireEvent.click(btn);
    await waitFor(() => expect(onDeleted).toHaveBeenCalled());
  });
});
