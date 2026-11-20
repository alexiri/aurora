import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ApprovalNotifications } from "@/components/account/approval-notifications";
import { MyKeysView } from "@/components/account/my-keys-view";
import { NotificationsView } from "@/components/account/notifications-view";
import { ProfileView } from "@/components/account/profile-view";
import type { Me } from "@/lib/api/queries";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-16 account-1 (blocked only after a try), account-2 (Create key), account-4 (Notifications has its own page).

vi.mock("@/components/account/push-devices", () => ({ PushDevices: () => <p>push devices</p> }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), usePathname: () => "/account", useSearchParams: () => new URLSearchParams() }));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

const me = {
  user: { id: "u1", email: "a@x.io", display_name: "Ann", is_instance_admin: false },
  approval_email: true,
  admin_attention_email: false,
  default_org_id: null,
  memberships: [
    { org: { id: "o1", slug: "acme", name: "Acme" }, role: "member", status: "active", sources: [], workspaces: [{ id: "w1", slug: "default", name: "Default", my_role: "owner", personal: false }] },
  ],
  groups: [],
} as unknown as Me;

function stubNotification(permission: NotificationPermission) {
  vi.stubGlobal("Notification", { permission, requestPermission: vi.fn(async () => permission) });
}

describe("Notifications blocked (account-1)", () => {
  it("a browser that blocks by default shows nothing until the person asks for notifications", async () => {
    stubNotification("denied");
    wrap(<ApprovalNotifications me={me} />);
    expect(screen.queryByText(/Notifications are blocked for this site/)).toBeNull();
    fireEvent.click(screen.getByLabelText(/Notify me in this browser when an approval needs me/));
    expect(await screen.findByText(/Notifications are blocked for this site/)).toBeInTheDocument();
  });

  it("says nothing when the browser allows them, and uses the words of the mockup", () => {
    stubNotification("default");
    wrap(<ApprovalNotifications me={me} />);
    expect(screen.queryByText(/blocked/)).toBeNull();
    expect(screen.getByText("Only the title and a link to the dashboard are sent.")).toBeInTheDocument();
    expect(screen.getByText("Works while a tab is open. Stored in this browser only.")).toBeInTheDocument();
  });
});

describe("Notifications page and the profile (account-4)", () => {
  it("the Notifications page holds the section and the push devices", async () => {
    stubNotification("default");
    mockFetch({ "GET /v1/me": () => json(200, me) });
    wrap(<NotificationsView />);
    expect(await screen.findByRole("heading", { name: "Approval notifications" })).toBeInTheDocument();
    expect(screen.getByText("How you hear about approvals that need a decision.")).toBeInTheDocument();
    expect(screen.getByText("push devices")).toBeInTheDocument();
  });

  it("the profile is only the profile: no notifications and no SSH link, with the line about what stays below", async () => {
    mockFetch({ "GET /v1/me": () => json(200, me), "GET /v1/instance": () => json(200, { version: "0", features: { ssh: true } }) });
    wrap(<ProfileView />);
    expect(await screen.findByText("Roles from sign-in, tour replays and appearance stay below.")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Approval notifications" })).toBeNull();
    expect(screen.queryByRole("link", { name: "Manage your SSH keys" })).toBeNull();
    expect(screen.getByText("Appearance")).toBeInTheDocument();
  });
});

describe("Create an API key on Account (account-2)", () => {
  it("opens a dialog with name and workspace, creates in that workspace and shows the secret once", async () => {
    const posts: { path: string; body: unknown }[] = [];
    mockFetch({
      "GET /v1/me/api-keys": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/me": () => json(200, me),
      "POST /v1/workspaces/w1/api-keys": async (req) => {
        posts.push({ path: "w1", body: await req.json() });
        return json(201, { api_key: { id: "k1", name: "My laptop CLI", kind: "service", org_id: "o1", workspace_id: "w1", display_prefix: "auk_ab", owner: { id: "u1", display_name: "Ann" }, created_at: "2026-01-01T00:00:00Z" }, key: "auk_secret" });
      },
    });
    wrap(<MyKeysView />);
    expect(await screen.findByText("You haven't created an API key")).toBeInTheDocument();
    expect(screen.getByText("Personal keys you created for workspaces (for example for the CLI).")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: "Create key" })[0]!);
    const dialog = await screen.findByRole("dialog", { name: "Create an API key" });
    await waitFor(() => expect(within(dialog).getByLabelText("Workspace")).toHaveDisplayValue("Default · Acme"));
    expect(within(dialog).getByText("The key can act only in this workspace.")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Create key" })).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "My laptop CLI" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create key" }));
    expect(await screen.findByTestId("secret-key")).toHaveTextContent("auk_secret");
    expect(posts).toEqual([{ path: "w1", body: { name: "My laptop CLI", expires_at: null } }]);
  });
});
