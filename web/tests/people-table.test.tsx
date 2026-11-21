import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AccessTab, SUSPEND_KEEPS, SUSPEND_STOPS } from "@/components/settings/access-tab";
import { formatExact, formatWhen } from "@/lib/format-date";
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

describe("Not signed in yet (workspace-4, C-1086)", () => {
  it("a person whose signed_in is false shows the chip, 'added …' and Copy link; one who has signed in shows the time", async () => {
    mockFetch({
      ...routes,
      "GET /v1/workspaces/w/access": () =>
        json(200, { ...access, items: [access.items[0], { ...access.items[1], joined_at: "2026-11-20T10:00:00Z", signed_in: false }] }),
    });
    wrap(<AccessTab wsId="w" orgId="o" canOwn people={{ workspacePath: "/o/acme/w/dev", initiallyOpen: false }} />);
    const cell = await screen.findByTestId("not-signed-in");
    expect(cell).toHaveTextContent("Not signed in yet");
    expect(cell).toHaveTextContent(/added /);
    expect(within(cell).getByRole("button", { name: "Copy link" })).toBeInTheDocument();
    expect(within(screen.getByTestId("access-row-me")).queryByTestId("not-signed-in")).toBeNull();
  });
});

describe("Promotion to Owner (workspace-5)", () => {
  it("asks only for Owner, with what an Owner can do; Approver applies at once; the select keeps the old value until Make Owner", async () => {
    const puts: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const req = input as Request;
        const url = new URL(req.url, "http://localhost");
        if (req.method === "PUT" && url.pathname.startsWith("/v1/workspaces/w/access/")) {
          puts.push(await req.json());
          return json(200, { ...access.items[1], role: "owner" });
        }
        const h = (routes as Record<string, () => Response>)[`${req.method} ${url.pathname}`];
        return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
      }),
    );
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    const select = (await screen.findByLabelText("Role for carol@x.io")) as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "approver" } });
    await waitFor(() => expect(puts).toEqual([{ role: "approver" }]));
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.change(select, { target: { value: "owner" } });
    const dialog = await screen.findByRole("dialog", { name: "Make Carol an Owner?" });
    expect(within(dialog).getByText("Member now → Owner")).toBeInTheDocument();
    for (const t of ["Add and remove people", "Change other people's roles", "Delete this workspace"]) expect(within(dialog).getByText(t)).toBeInTheDocument();
    expect(within(dialog).getByText("You can change them back to Member at any time.")).toBeInTheDocument();
    expect(select.value).toBe("member");
    expect(puts).toHaveLength(1);
    fireEvent.click(within(dialog).getByRole("button", { name: "Make Owner" }));
    await waitFor(() => expect(puts).toHaveLength(2));
    expect(puts[1]).toEqual({ role: "owner" });
  });
});

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

  it("(batch 8, b7#8c) a person stored by their address is named by it once, as the address: Erin, with erin@… under it", async () => {
    mockFetch({
      ...routes,
      "GET /v1/workspaces/w/access": () => json(200, { ...access, items: [{ user_id: "e", email: "erin@example.test", display_name: "erin@example.test", role: "member", implicit: false }] }),
    });
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    const select = await screen.findByLabelText("Role for erin@example.test");
    const row = select.closest("tr")!;
    expect(row.textContent).toContain("Erin");
    expect(row.textContent!.match(/erin@example\.test/g)).toHaveLength(1);
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

  it("shows when each person joined, with the exact time as the tooltip and a dash for access from the organization", async () => {
    mockFetch({
      ...routes,
      "GET /v1/workspaces/w/access": () =>
        json(200, {
          ...access,
          items: [
            { ...access.items[0], joined_at: null, status: "active" },
            { ...access.items[1], joined_at: "2025-03-04T10:00:00Z", status: "active" },
          ],
        }),
    });
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    expect(await screen.findByRole("columnheader", { name: "Joined" })).toBeInTheDocument();
    const row = screen.getByTestId("access-row-c");
    const cell = within(row).getByTitle(formatExact("2025-03-04T10:00:00Z"));
    expect(cell).toHaveTextContent(formatWhen("2025-03-04T10:00:00Z"));
    // The same cell is the "Joined …" line of the stacked row on a phone (its label comes from the stylesheet).
    expect(cell.closest("td")).toHaveAttribute("data-label", "Joined");
  });

  it("suspends after a confirmation that says what stops, marks the row, and restores", async () => {
    const calls: { key: string; body: unknown }[] = [];
    let carol = { ...access.items[1], joined_at: "2025-03-04T10:00:00Z", status: "active" };
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
        const url = new URL(req.url, "http://localhost");
        const key = `${req.method} ${url.pathname}`;
        const text = req.method === "GET" ? "" : await req.clone().text();
        const body = text ? JSON.parse(text) : undefined;
        calls.push({ key, body });
        if (key === "PATCH /v1/workspaces/w/access/c") {
          carol = { ...carol, status: (body as { status: string }).status };
          return json(200, carol);
        }
        if (key === "GET /v1/workspaces/w/access") return json(200, { ...access, items: [access.items[0], carol] });
        const h = (routes as Record<string, () => Response>)[key];
        return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
      }),
    );
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    fireEvent.keyDown(await screen.findByRole("button", { name: "More for Carol" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Suspend carol@x.io" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Suspend Carol in this workspace?")).toBeInTheDocument();
    expect(within(dialog).getByText(SUSPEND_STOPS)).toBeInTheDocument();
    expect(within(dialog).getByText(SUSPEND_KEEPS)).toBeInTheDocument();
    expect(calls.some((c) => c.key.startsWith("PATCH"))).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Suspend" }));
    await waitFor(() => expect(calls.find((c) => c.key === "PATCH /v1/workspaces/w/access/c")?.body).toEqual({ status: "suspended" }));
    const row = await screen.findByTestId("access-row-c");
    expect(await within(row).findByText("Suspended")).toBeInTheDocument();
    fireEvent.keyDown(within(row).getByRole("button", { name: "More for Carol" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Restore carol@x.io" }));
    await waitFor(() => expect(calls.filter((c) => c.key === "PATCH /v1/workspaces/w/access/c").at(-1)?.body).toEqual({ status: "active" }));
    await waitFor(() => expect(within(screen.getByTestId("access-row-c")).queryByText("Suspended")).toBeNull());
  });

  it("does not mark an organization owner suspended: their access comes from the organization", async () => {
    mockFetch({
      ...routes,
      "GET /v1/workspaces/w/access": () =>
        json(200, { ...access, items: [access.items[0], { user_id: "z", email: "zed@x.io", display_name: "Zed", role: "owner", implicit: true, status: "suspended", joined_at: null }] }),
    });
    wrap(<AccessTab wsId="w" orgId="o" canOwn />);
    const row = await screen.findByTestId("access-row-z");
    expect(within(row).queryByText("Suspended")).toBeNull();
    expect(within(row).queryByRole("button")).toBeNull();
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
