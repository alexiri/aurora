import { screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AccessTab } from "@/components/settings/access-tab";
import { json } from "./support/api";
import { wrap } from "./support/render";

// (P5-POLISH-7, D6) One People table: your own row has no menu and no role change, the last owner's role cannot be lowered,
// people who have access through the organization or a group are rows with their reason and no menu, the role legend is a
// disclosure under the table, and a personal workspace has no Groups.

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

const access = {
  items: [
    { user_id: "me", email: "ada@x.io", display_name: "Ada", role: "owner", implicit: false },
    { user_id: "c", email: "carol@x.io", display_name: "Carol", role: "member", implicit: false },
  ],
  derived_members: {
    items: [{ user: { id: "o", email: "olga@x.io", display_name: "Olga" }, role: "owner", sources: [{ kind: "org_role" }] }],
    next_cursor: null,
  },
  group_bindings: [],
  next_cursor: null,
};
const routes = {
  "GET /v1/workspaces/w/access": () => json(200, access),
  "GET /v1/orgs/o/members": () => json(200, { items: [], next_cursor: null }),
  "GET /v1/orgs/o/groups": () => json(200, { items: [], next_cursor: null }),
  "GET /v1/me": () => json(200, { user: { id: "me", display_name: "Ada", email: "ada@x.io", is_instance_admin: false }, memberships: [], default_org_id: null }),
};

describe("People table", () => {
  it("has no menu on your own row, a menu on others, a reason on inherited access and sentence-case roles", async () => {
    mockFetch(routes);
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    expect(await screen.findByText("(you)")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "More for Ada" })).toBeNull();
    expect(screen.getByRole("button", { name: "More for Carol" })).toBeInTheDocument();
    const carol = screen.getByLabelText("Role for carol@x.io");
    expect(within(carol).getAllByRole("option").map((o) => o.textContent)).toEqual(["Member", "Approver", "Owner"]);
    const inherited = screen.getByTestId("inherited-row");
    expect(inherited).toHaveTextContent("Olga");
    expect(within(inherited).queryByRole("button")).toBeNull();
    // Your own role is text, not a select.
    expect(screen.queryByLabelText("Role for ada@x.io")).toBeNull();
    expect(screen.getByText("What can each role do?")).toBeInTheDocument();
  });

  it("does not offer a lower role for the last owner", async () => {
    mockFetch({
      ...routes,
      "GET /v1/workspaces/w/access": () => json(200, { ...access, items: [{ user_id: "x", email: "sam@x.io", display_name: "Sam", role: "owner", implicit: false }] }),
    });
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    const sam = await screen.findByLabelText("Role for sam@x.io");
    expect(within(sam).getAllByRole("option").map((o) => o.textContent)).toEqual(["Owner"]);
  });

  it("shows Groups in a shared workspace and not in a personal one", async () => {
    mockFetch(routes);
    const { unmount } = wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    expect(await screen.findByText("Groups")).toBeInTheDocument();
    unmount();
    wrap(<AccessTab wsId="w" orgId="o" canOwn personal />);
    await screen.findByText("(you)");
    expect(screen.queryByText("Groups")).toBeNull();
  });
});
