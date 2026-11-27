import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { personalWorkspaceSentence } from "@/components/org/invite-dialog";
import { OrgSkills } from "@/components/skills/org-skills";
import { pickRailWorkspace } from "@/lib/nav/manage-return";
import { lastScope, writeRecent } from "@/lib/nav/recent-scopes";
import { json } from "./support/api";
import { wrap } from "./support/render";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme" }),
  useSearchParams: () => new URLSearchParams(),
  usePathname: () => "/o/acme/agents",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
}));

type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${decodeURIComponent(url.pathname)}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

describe("P5-REFINE-20 batch 3 (workspace-7: one personal-workspace name)", () => {
  const memberships = [{ org: { slug: "acme", name: "Acme" }, status: "active", workspaces: [{ slug: "team", name: "Team" }, { slug: "alice", name: "Alice's workspace", personal: true }] }];

  it("the Manage back link and the last scope call your personal workspace 'Your workspace', as the switcher does", () => {
    expect(pickRailWorkspace("acme", null, memberships)).toEqual({ wsSlug: "alice", wsName: "Your workspace" });
    window.localStorage.clear();
    writeRecent("u1", "acme/alice");
    expect(lastScope("u1", memberships)?.wsName).toBe("Your workspace");
    writeRecent("u1", "acme/team");
    expect(lastScope("u1", memberships)?.wsName).toBe("Team");
  });
});

describe("P5-REFINE-20 batch 3 (organization words)", () => {
  it("org-11: the personal workspace sentence agrees with its subject", () => {
    expect(personalWorkspaceSentence("")).toBe("They get their own personal workspace automatically.");
    expect(personalWorkspaceSentence("dana@example.test")).toBe("Dana gets their own personal workspace automatically.");
  });

  it("org-12: the organization's Skills page leads with its heading, and the precedence sits in a closed 'How skills combine' fold", async () => {
    mockFetch({
      "GET /v1/resolve": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: null }),
      "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", my_role: "owner", contacts: [] }),
      "GET /v1/orgs/o1/skills": () => json(200, { items: [], next_cursor: null }),
    });
    wrap(<OrgSkills />);
    expect(await screen.findByRole("heading", { level: 1, name: "Skills" })).toBeTruthy();
    const fold = screen.getByTestId("skills-combine");
    expect(fold).not.toHaveAttribute("open");
    expect(fold).toHaveTextContent("A workspace skill or an agent's own skill with the same name overrides an organization skill");
    // The paragraph is not a notice under the heading any more.
    for (const n of screen.queryAllByRole("status")) expect(n.textContent ?? "").not.toMatch(/with the same name overrides/);
    // The fold comes after the list, not before it.
    const heading = screen.getByRole("heading", { level: 1, name: "Skills" });
    expect(heading.compareDocumentPosition(fold) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(fold.querySelector("summary")!);
  });
});
