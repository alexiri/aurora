import { screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import WorkspaceRedirect from "@/app/o/[org]/w/[ws]/layout";
import OrgWorkspacesPage from "@/app/o/[org]/workspaces/page";
import { useWorkspace } from "@/lib/api/queries";
import { wrap } from "./support/render";
import { json, type Handler } from "./support/api";

// (FIX-235, D-477) The org Overview links the workspaces the person is in as before and the
// others to the manage page (quiet "Manage" label); an old workspace address whose Workspace
// says `my_role: null` with `administers: true` redirects an administrator there.

const replace = vi.fn();
let path = "/o/acme/w/lab";
beforeEach(() => {
  path = "/o/acme/w/lab";
});
vi.mock("next/navigation", () => ({
  usePathname: () => path,
  useParams: () => ({ org: "acme", ws: "lab" }),
  useRouter: () => ({ push: vi.fn(), replace }),
}));
vi.mock("next/link", () => ({
  default: ({ href, children, ...rest }: { href: string; children: ReactNode }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));

/** A page under the layout that shows only once the workspace has answered, so "it stayed" is the layout's own decision. */
function Body() {
  return useWorkspace("w1").data ? <p>page body</p> : null;
}

const now = "2026-11-20T10:00:00Z";
const wsRow = (slug: string, name: string, my_role: string | null, over: Record<string, unknown> = {}) => ({
  id: `w-${slug}`, org_id: "o1", slug, name, my_role, administers: my_role == null,
  memory_language: "en", timezone: "UTC", created_at: now, updated_at: now, personal_of: null, ...over,
});

function serve(routes: Record<string, Handler>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const h = routes[`${req?.method ?? "GET"} ${url.pathname}`];
      return h ? h(req as Request) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

beforeEach(() => {
  replace.mockClear();
  vi.unstubAllGlobals();
});

// (D-508(c)) The list moved from the Overview to Manage <org> › Workspaces.
describe("Manage › Workspaces rows (FIX-235)", () => {
  it("one of this person's own workspaces links into the workspace, a managed one to the manage page", async () => {
    serve({
      "GET /v1/resolve": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" } }),
      "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", my_role: "admin" }),
      "GET /v1/me": () => json(200, { user: { id: "me", display_name: "Ada", email: "ada@x.io", is_instance_admin: false }, memberships: [], default_org_id: null }),
      "GET /v1/orgs/o1/workspaces": () =>
        json(200, {
          items: [
            wsRow("mine", "My team", "member"),
            wsRow("other", "Ops", null),
            wsRow("bob", "Bob's workspace", null, { personal_of: { id: "p1", name: "Bob Builder" } }),
          ],
          next_cursor: null,
        }),
    });
    wrap(<OrgWorkspacesPage />);
    const own = await screen.findByRole("link", { name: "My team" });
    expect(own).toHaveAttribute("href", "/o/acme/w/mine");
    const managed = screen.getByRole("link", { name: /Ops/ });
    expect(managed).toHaveAttribute("href", "/o/acme/workspaces/other");
    expect(within(managed).getByTestId("manage-row")).toHaveTextContent("Manage");
    const personal = screen.getByRole("link", { name: "Bob's workspace Manage" });
    expect(personal).toHaveAttribute("href", "/o/acme/workspaces/bob");
    expect(within(personal).getByTestId("manage-row")).toBeInTheDocument();
  });
});

describe("the old-address redirect (FIX-235)", () => {
  const routesFor = (over: Record<string, unknown>) => ({
    "GET /v1/resolve": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: { id: "w1", slug: "lab", name: "Lab" } }),
    "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", slug: "lab", name: "Lab", my_role: null, administers: true, memory_language: "en", timezone: "UTC", created_at: now, updated_at: now, personal_of: null, frozen: null, ...over }),
  });

  it("my_role null with administers redirects to the manage page and shows nothing else", async () => {
    serve(routesFor({}));
    wrap(<WorkspaceRedirect><Body /></WorkspaceRedirect>);
    await waitFor(() => expect(replace).toHaveBeenCalledWith("/o/acme/workspaces/lab"));
    expect(screen.queryByText("page body")).toBeNull();
  });

  // (FIX-277, D-478) The real answer to an emergency reader is `my_role: null, administers: true`:
  // the grant, not the administration, decides. They stay on the page they asked for.
  it("an emergency reader, who also administers, stays on the page they asked for", async () => {
    serve(routesFor({ emergency_reader: true }));
    wrap(<WorkspaceRedirect><Body /></WorkspaceRedirect>);
    await screen.findByText("page body");
    expect(replace).not.toHaveBeenCalled();
  });

  it("an administrator whose grant is over (emergency_reader false) is redirected again", async () => {
    serve(routesFor({ emergency_reader: false }));
    wrap(<WorkspaceRedirect><Body /></WorkspaceRedirect>);
    await waitFor(() => expect(replace).toHaveBeenCalledWith("/o/acme/workspaces/lab"));
    expect(screen.queryByText("page body")).toBeNull();
  });

  // (FIX-277) A page the grant doesn't cover never mounts for a reader: nothing there can act or ask.
  it("a reader on a page the grant doesn't cover sees that, and the page never mounts", async () => {
    path = "/o/acme/w/lab/settings";
    serve(routesFor({ emergency_reader: true }));
    wrap(<WorkspaceRedirect><Body /></WorkspaceRedirect>);
    const notice = await screen.findByTestId("emergency-not-covered");
    expect(notice).toHaveTextContent("Emergency read access doesn’t cover this page.");
    expect(within(notice).getByRole("link", { name: "Read the workspace’s agents" })).toHaveAttribute("href", "/o/acme/w/lab/agents");
    expect(screen.queryByText("page body")).toBeNull();
    expect(replace).not.toHaveBeenCalled();
  });

  it("a member reads that same page as ever", async () => {
    path = "/o/acme/w/lab/settings";
    serve(routesFor({ my_role: "member", administers: false }));
    wrap(<WorkspaceRedirect><Body /></WorkspaceRedirect>);
    await screen.findByText("page body");
    expect(screen.queryByTestId("emergency-not-covered")).toBeNull();
  });

  it("own access and a plain member render the page", async () => {
    serve(routesFor({ my_role: "member", administers: false }));
    wrap(<WorkspaceRedirect><Body /></WorkspaceRedirect>);
    await screen.findByText("page body");
    expect(replace).not.toHaveBeenCalled();
  });

  it("administers false with my_role null (a member of another workspace view) does not redirect", async () => {
    serve(routesFor({ administers: false }));
    wrap(<WorkspaceRedirect><Body /></WorkspaceRedirect>);
    await screen.findByText("page body");
    expect(replace).not.toHaveBeenCalled();
  });
});
