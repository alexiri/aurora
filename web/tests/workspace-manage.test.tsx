import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { WorkspaceManage } from "@/components/workspace/workspace-manage";
import { wrap } from "./support/render";
import { json, type Handler } from "./support/api";

// (FIX-235, D-477) The manage page: an organization admin who is not one of the workspace's
// people gets the notice, the People table, the agents with Pause/Resume (no link into an
// agent), the freeze/danger controls, and "Add yourself" as a danger step that PUTs the chosen
// role to access/{me} (C-1161) — refused on a personal workspace (D-480).

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "lab" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
}));
vi.mock("next/link", () => ({
  default: ({ href, children, ...rest }: { href: string; children: ReactNode }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));

const now = "2026-11-20T10:00:00Z";
const ws = (over: Record<string, unknown> = {}) => ({
  id: "w1", org_id: "o1", slug: "lab", name: "Lab", my_role: null, administers: true,
  memory_language: "en", timezone: "UTC", created_at: now, updated_at: now, personal_of: null, frozen: null, ...over,
});
const agent = (id: string, name: string, over: Record<string, unknown> = {}) => ({
  id, org_id: "o1", workspace_id: "w1", handle: name.toLowerCase(), name, kind: "hosted",
  state: "ready", is_default: false, frozen: null, created_at: now, updated_at: now, ...over,
});
const FROZEN = { at: now, authority: "workspace_owner", by: { id: "me", display_name: "Ada" }, by_label: "ada@x.io", level: "agent", reason: "test" };
const access = {
  items: [{ user_id: "u1", email: "carol@x.io", display_name: "Carol", role: "member", implicit: false, status: "active", joined_at: now, signed_in: true }],
  derived_members: { items: [], next_cursor: null },
  group_bindings: [],
  next_cursor: null,
};

function serve(over: Record<string, Handler> = {}, wsOver: Record<string, unknown> = {}) {
  const calls: { method: string; path: string; body: unknown }[] = [];
  const routes: Record<string, Handler> = {
    "GET /v1/resolve": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: { id: "w1", slug: "lab", name: "Lab" } }),
    "GET /v1/workspaces/w1": () => json(200, ws(wsOver)),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [agent("a1", "Daneel"), agent("a2", "Rex", { frozen: FROZEN })], next_cursor: null }),
    "GET /v1/workspaces/w1/access": () => json(200, access),
    "GET /v1/workspaces/w1/access/pending": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", my_role: "owner" }),
    "GET /v1/orgs/o1/members": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/orgs/o1/groups": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/me": () => json(200, { user: { id: "me", display_name: "Ada", email: "ada@x.io", is_instance_admin: false }, memberships: [], default_org_id: null }),
    "PUT /v1/workspaces/w1/access/me": () => json(200, { ...access.items[0], user_id: "me", role: "owner" }),
    ...over,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const key = `${req?.method ?? "GET"} ${url.pathname}`;
      if (req && req.method !== "GET") {
        let body: unknown = null;
        try {
          body = await req.json();
        } catch {
          body = null;
        }
        calls.push({ method: req.method, path: url.pathname, body });
      }
      const h = routes[key];
      return h ? h(req as Request) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

beforeEach(() => {
  vi.unstubAllGlobals();
});

describe("Workspace manage page (FIX-235, D-477)", () => {
  it("says why the page is here, lists the people and the agents without links into them", async () => {
    serve();
    wrap(<WorkspaceManage />);
    const notice = await screen.findByTestId("manage-notice");
    expect(notice).toHaveTextContent("You manage this workspace as an organization admin.");
    expect(notice).toHaveTextContent("You’re not one of its people, so you can’t open its chats or use its agents.");
    // An org owner is told about the emergency read.
    expect(notice).toHaveTextContent("In an emergency you can open read access.");
    expect(await screen.findByText("Carol")).toBeInTheDocument();
    // The agents: state and Pause, the name never a link.
    expect(await screen.findByText("Daneel")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause…" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Resume/ })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Daneel" })).toBeNull();
    // The danger controls are present.
    expect(screen.getByText("Danger zone")).toBeInTheDocument();
  });

  it("a personal workspace has no Add people and names its owner instead", async () => {
    serve({}, { personal_of: { id: "p1", name: "Bob Builder" } });
    wrap(<WorkspaceManage />);
    const notice = await screen.findByTestId("manage-notice");
    expect(notice).toHaveTextContent("Only Bob Builder can add people to it.");
    expect(screen.queryByRole("button", { name: "Add people" })).toBeNull();
    expect(screen.getByText("Personal")).toBeInTheDocument();
  });

  it("an administrator who is not an org owner is not offered the emergency read", async () => {
    serve({ "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", my_role: "admin" }) });
    wrap(<WorkspaceManage />);
    const notice = await screen.findByTestId("manage-notice");
    expect(notice).toHaveTextContent("You manage this workspace as an organization admin.");
    expect(notice).not.toHaveTextContent("In an emergency");
  });

  it("Add yourself asks first, says what it does, and PUTs the chosen role", async () => {
    const calls = serve();
    wrap(<WorkspaceManage />);
    fireEvent.click(await screen.findByTestId("add-yourself"));
    const dialog = await screen.findByRole("dialog", { name: "Add yourself to Lab?" });
    expect(dialog).toHaveTextContent("You'll be able to use its agents and their accounts, and as an owner read its people's private chats.");
    expect(dialog).toHaveTextContent("Its owners will be told.");
    // Member is the default; switch to owner.
    fireEvent.click(within(dialog).getByLabelText("Owner"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Add yourself to Lab" }));
    await waitFor(() =>
      expect(calls).toEqual([{ method: "PUT", path: "/v1/workspaces/w1/access/me", body: { role: "owner" } }]),
    );
    // The dialog closes on success.
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("a personal workspace refuses adding yourself (D-480) and says so", async () => {
    const calls = serve(
      {
        "PUT /v1/workspaces/w1/access/me": () =>
          json(403, { error: { code: "personal_workspace_owner_only", message: "Only the owner of a personal workspace can be added to it." } }),
      },
      { personal_of: { id: "p1", name: "Bob Builder" } },
    );
    wrap(<WorkspaceManage />);
    fireEvent.click(await screen.findByTestId("add-yourself"));
    const dialog = await screen.findByRole("dialog", { name: "Add yourself to Lab?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add yourself to Lab" }));
    await waitFor(() => expect(calls).toHaveLength(1));
    // The refusal is said in words (the server's message) and the dialog stays open: nothing was granted.
    const dialog2 = screen.getByRole("dialog", { name: "Add yourself to Lab?" });
    expect(await within(dialog2).findByRole("alert")).toHaveTextContent("Only the owner of a personal workspace can be added to it.");
    expect(screen.getByRole("dialog", { name: "Add yourself to Lab?" })).toBeInTheDocument();
  });
});
