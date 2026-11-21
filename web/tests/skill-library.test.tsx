import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { SkillsEditor } from "@/components/skills/skills-view";
import { diffLines } from "@/lib/api/skill-libraries";
import type { Skill } from "@/lib/api/skills";
import { wrap } from "./support/render";
import { json } from "./support/api";

// P4-GSL (D-148; docs/14 Skill libraries, Add from library, update review; docs/18 Dashboard): Add from
// library, library badges and read-only skills, detach and remove, the update review with the whole diff,
// and repository text rendered as data. The libraries pages and their dialogs (P5-SKLIB-3) are in
// skill-libraries-ui.test.tsx.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme" }),
  useSearchParams: () => new URLSearchParams(),
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

const T = "2026-01-01T00:00:00Z";
const SHA_A = "0123456789abcdef0123456789abcdef01234567";
const SHA_B = "89abcdef0123456789abcdef0123456789abcdef";
const HOSTILE = "Evil <img src=x onerror=alert(1)> [click](javascript:alert(1))";

// (P5-SKLIB-1, C-1122) A library is a git address: `url` and its display form `repo`; no app, no connection.
const library = (over: Record<string, unknown> = {}) => ({
  id: "l1", scope: "org", org_id: "o1", label: "Team skills", status: "active", url: "https://git.example.org/acme/agent-skills.git",
  repo: "git.example.org/acme/agent-skills", ref: null, path: "skills", token_set: false, token_updated_at: null,
  last_head_sha: SHA_B, last_refresh_at: T, last_error: null,
  skill_counts: { agent: 1, workspace: 2, org: 3 }, pending_updates: 2, created_at: T, ...over,
});

function skill(over: Partial<Skill> & Pick<Skill, "id" | "name" | "tier">): Skill {
  return {
    scope: over.tier === "org" ? "organization" : over.tier,
    agent_id: over.tier === "agent" ? "a1" : null,
    workspace_id: over.tier === "workspace" ? "w1" : null,
    org_id: "o1",
    description: `${over.name} description`,
    body: `${over.name} body\n`,
    enabled: true,
    files: [],
    total_size: 20,
    overridden_in: null,
    source: null,
    created_at: T,
    updated_at: T,
    ...over,
  } as Skill;
}
const source = (over: Record<string, unknown> = {}) => ({
  kind: "library", library_id: "l1", label: "Team skills", repo: "git.example.org/acme/agent-skills", path: "code-review", commit_sha: SHA_A, pinned_at: T, detached: false, update_pending: false, ...over,
});
const libSkill = (over: Partial<Skill> = {}) =>
  skill({ id: "s1", name: "code-review", tier: "org", source: source() as Skill["source"], ...over });

// ---- library skills in the editor --------------------------------------------------------------

describe("library skills", () => {
  const skills = (items: Skill[]) => ({
    "GET /v1/orgs/o1/skills": () => json(200, { items, next_cursor: null }),
    "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [library()] }),
    "GET /v1/orgs/o1/skill-library-updates": () => json(200, { items: [] }),
  });

  it("shows the badge, the read-only banner, and saves only the enabled flag", async () => {
    let patched: unknown = null;
    mockFetch({
      ...skills([libSkill()]),
      "PATCH /v1/skills/s1": async (r) => {
        patched = await r.json();
        return json(200, libSkill({ enabled: false }));
      },
    });
    wrap(<SkillsEditor scope={{ kind: "organization", id: "o1" }} orgId="o1" />);
    expect(await screen.findByText("Library · git.example.org/acme/agent-skills · 0123456")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Code review/ }));
    expect(await screen.findByText("Managed by a skill library; edit it in the repository or detach it.")).toBeTruthy();
    expect((screen.getByLabelText("Use it when…") as HTMLInputElement).readOnly).toBe(true);
    expect((screen.getByLabelText("Instructions") as HTMLTextAreaElement).readOnly).toBe(true);
    fireEvent.click(screen.getByLabelText("On for the organization's agents"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(patched).toEqual({ enabled: false }));
    // remove, not delete
    expect(screen.getByRole("button", { name: "Remove" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("detach asks first and then converts the skill", async () => {
    let detached = false;
    mockFetch({
      ...skills([libSkill()]),
      "POST /v1/skills/s1/library/detach": () => {
        detached = true;
        return json(200, libSkill({ source: source({ detached: true, library_id: null }) as Skill["source"] }));
      },
    });
    wrap(<SkillsEditor scope={{ kind: "organization", id: "o1" }} orgId="o1" />);
    fireEvent.click(await screen.findByRole("button", { name: /Code review/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Detach" }));
    expect(screen.getByText(/ordinary editable skill/)).toBeTruthy();
    expect(detached).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Confirm detach" }));
    await waitFor(() => expect(detached).toBe(true));
  });

  it("a detached skill is editable again and keeps its provenance note", async () => {
    mockFetch(skills([libSkill({ source: source({ detached: true, library_id: null }) as Skill["source"] })]));
    wrap(<SkillsEditor scope={{ kind: "organization", id: "o1" }} orgId="o1" />);
    fireEvent.click(await screen.findByRole("button", { name: /Code review/ }));
    expect((await screen.findByLabelText("Use it when…") as HTMLInputElement).readOnly).toBe(false);
    expect(screen.getAllByText(/Detached · was git\.example\.org\/acme\/agent-skills@0123456/).length).toBeGreaterThan(0);
    expect(screen.queryByText(/Managed by a skill library/)).toBeNull();
  });
});

// ---- update review --------------------------------------------------------------------------

describe("update review", () => {
  const review = (over: Record<string, unknown> = {}) => ({
    library: { id: "l1", label: "Team skills", repo: "git.example.org/acme/agent-skills", path: "code-review" }, commit_sha: SHA_A, pinned_at: T, pinned_by: null, detached: false,
    update: {
      id: "u1", kind: "changed", commit_sha: SHA_B, created_at: T, content_sha256: "f".repeat(64), over_budget: false, reason: null,
      diff: {
        description: { old: "Review", new: HOSTILE, changed: true },
        body: { old: "line one\nline two\n", new: "line one\nline two changed\nline three\n", changed: true },
        files: [{ path: "check.md", change: "added", old: null, new: "- tests\n", old_executable: null, new_executable: false, binary: false }],
      },
      ...over,
    },
  });
  const open = async (readOnly = false) => {
    wrap(<SkillsEditor scope={{ kind: "organization", id: "o1" }} orgId="o1" readOnly={readOnly} />);
    fireEvent.click(await screen.findByRole("button", { name: /Code review/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Review update" }));
    const dlg = await screen.findByRole("dialog");
    // the update is loaded when its buttons exist
    await within(dlg).findByRole("button", { name: "Approve" });
    return dlg;
  };
  const pendingSkill = () => libSkill({ source: source({ update_pending: true }) as Skill["source"] });
  const base = (info: Handler, extra: Record<string, Handler> = {}) => ({
    "GET /v1/orgs/o1/skills": () => json(200, { items: [pendingSkill()], next_cursor: null }),
    "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [library()] }),
    "GET /v1/orgs/o1/skill-library-updates": () => json(200, { items: [{ id: "u1", kind: "changed", created_at: T, skill: { id: "s1", name: "code-review", tier: "org" }, library: { id: "l1", label: "Team skills" } }] }),
    "GET /v1/skills/s1/library": info,
    ...extra,
  });

  it("flags the update, shows the whole diff, and approve quotes the reviewed content hash", async () => {
    let approved: unknown = null;
    mockFetch(base(() => json(200, review()), {
      "POST /v1/skills/s1/library/updates/u1/approve": async (r) => {
        approved = await r.json();
        return json(200, pendingSkill());
      },
    }));
    const dlg = await open();
    expect(screen.getAllByText("Update available").length).toBeGreaterThan(0);
    expect(within(dlg).getByText("Changed")).toBeTruthy();
    expect(within(dlg).getByText("0123456")).toBeTruthy();
    expect(within(dlg).getByText("89abcde")).toBeTruthy();
    // every line, nothing summarized
    const body = within(dlg).getByLabelText("SKILL.md diff");
    expect(body.textContent).toContain("  line one");
    expect(body.textContent).toContain("- line two");
    expect(body.textContent).toContain("+ line two changed");
    expect(body.textContent).toContain("+ line three");
    expect(within(dlg).getByLabelText("Diff of check.md").textContent).toContain("+ - tests");
    // repository text is data: no element, no link, the payload is visible as text
    expect(dlg.querySelector("img")).toBeNull();
    expect(dlg.querySelector("a")).toBeNull();
    expect(within(dlg).getByLabelText("Description diff").textContent).toContain("<img src=x onerror=alert(1)>");
    fireEvent.click(within(dlg).getByRole("button", { name: "Approve" }));
    await waitFor(() => expect(approved).toEqual({ content_sha256: "f".repeat(64) }));
  });

  it("a person without the tier's authority reads the diff but cannot approve, with the reason", async () => {
    mockFetch(base(() => json(200, review())));
    const dlg = await open(true);
    expect(within(dlg).getByLabelText("SKILL.md diff")).toBeTruthy();
    expect((within(dlg).getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    expect((within(dlg).getByRole("button", { name: "Reject" }) as HTMLButtonElement).disabled).toBe(true);
    expect(within(dlg).getByText(/Only someone who can be an organization owner or admin can approve or reject it/)).toBeTruthy();
  });

  it("an update the repository broke can only be rejected; a removed skill says what approving does", async () => {
    mockFetch(base(() => json(200, review({ kind: "invalid", diff: null, reason: "a symlink is not allowed (link)" }))));
    const dlg = await open();
    expect(within(dlg).getByText(/a symlink is not allowed \(link\)/)).toBeTruthy();
    expect((within(dlg).getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    expect((within(dlg).getByRole("button", { name: "Reject" }) as HTMLButtonElement).disabled).toBe(false);
    cleanup();
    mockFetch(base(() => json(200, review({ kind: "removed" }))));
    const dlg2 = await open();
    expect(within(dlg2).getByText(/Approving removes it from this organization/)).toBeTruthy();
  });

  it("over budget disables approve with the numbers; a superseded update offers a reload", async () => {
    mockFetch(base(() => json(200, review({ over_budget: true }))));
    const dlg = await open();
    expect((within(dlg).getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    expect(within(dlg).getByText(/100 skills, 4096 KiB/)).toBeTruthy();
    cleanup();
    mockFetch(base(() => json(200, review()), {
      "POST /v1/skills/s1/library/updates/u1/approve": () => json(409, { error: { code: "update_superseded", message: "x" } }),
    }));
    const dlg2 = await open();
    fireEvent.click(within(dlg2).getByRole("button", { name: "Approve" }));
    expect(await within(dlg2).findByText(/changed or was decided in the meantime/)).toBeTruthy();
    expect(within(dlg2).getByRole("button", { name: "Reload the update" })).toBeTruthy();
  });
});

// ---- Add from library -----------------------------------------------------------------------

describe("Add from library", () => {
  const catalogue = {
    commit_sha: SHA_B, truncated: false, next_cursor: null,
    items: [
      { path: "code-review", name: "code-review", description: HOSTILE, valid: true, reason: null, size: 500, files: 2, added: [{ tier: "org", id: "s9" }] },
      { path: "release-notes", name: "release-notes", description: "Write release notes", valid: true, reason: null, size: 900, files: 1, added: [] },
      { path: "Bad_Name", name: "Bad_Name", description: null, valid: false, reason: "the directory name is not a valid skill name", size: 10, files: 1, added: [] },
    ],
  };
  const content = {
    commit_sha: SHA_B, path: "release-notes", name: "release-notes", valid: true, reason: null, description: "Write release notes",
    body: "Collect merged pull requests.\n", files: [{ path: "template.md", size: 12, executable: false, binary: false, content: "# Notes", content_b64: null }], size: 40, content_sha256: "a".repeat(64),
  };
  const routes = (extra: Record<string, Handler> = {}) => ({
    "GET /v1/orgs/o1/skills": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [{ id: "l1", label: "Team skills", status: "active" }] }),
    "GET /v1/skill-libraries/l1/skills": () => json(200, catalogue),
    "GET /v1/skill-libraries/l1/skills/release-notes": () => json(200, content),
    ...extra,
  });

  it("is absent when the org has no active library", async () => {
    mockFetch({ ...routes(), "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [{ id: "l1", label: "Paused", status: "paused" }] }) });
    wrap(<SkillsEditor scope={{ kind: "organization", id: "o1" }} orgId="o1" />);
    await screen.findByRole("button", { name: "New skill" });
    expect(screen.queryByRole("button", { name: "Add from library…" })).toBeNull();
  });

  it("lists skills at the commit, disables invalid ones with the reason, previews the exact content and adds it pinned", async () => {
    let added: unknown = null;
    mockFetch(routes({
      "POST /v1/orgs/o1/skills": async (r) => {
        added = await r.json();
        return json(201, libSkill({ id: "new", name: "release-notes" }));
      },
    }));
    wrap(<SkillsEditor scope={{ kind: "organization", id: "o1" }} orgId="o1" />);
    fireEvent.click(await screen.findByRole("button", { name: "Add from library…" }));
    const list = await screen.findByRole("list", { name: "Library skills" });
    expect(within(list).getByText("Already added · Organization")).toBeTruthy();
    expect((within(list).getByRole("button", { name: /^Bad_Name/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(within(list).getByText(/Cannot be added: the directory name is not a valid skill name/)).toBeTruthy();
    // hostile description text is data
    expect(list.querySelector("img")).toBeNull();
    expect(within(list).getByText(/<img src=x onerror=alert\(1\)>/)).toBeTruthy();
    fireEvent.click(within(list).getByRole("button", { name: /^release-notes/ }));
    const preview = await screen.findByLabelText("Preview of what will be stored");
    expect(within(preview).getByLabelText("SKILL.md body").textContent).toContain("Collect merged pull requests.");
    expect(within(preview).getByLabelText("File template.md").textContent).toBe("# Notes");
    expect(screen.getByText("Pinned to this commit. Changes in the repository are not applied until someone with permission approves them.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() => expect(added).toEqual({ from_library: { library_id: "l1", path: "release-notes", commit_sha: SHA_B } }));
  });

  it("lists every catalogue page: Load more follows next_cursor (FIX-100)", async () => {
    const mk = (from: number, n: number) =>
      Array.from({ length: n }, (_, i) => ({ path: `s-${from + i}`, name: `s-${from + i}`, description: "d", valid: true, reason: null, size: 10, files: 1, added: [] }));
    const cursors: (string | null)[] = [];
    const orgs = new Set<string | null>();
    mockFetch(routes({
      "GET /v1/skill-libraries/l1/skills": (r) => {
        const params = new URL(r.url, "http://localhost").searchParams;
        const cursor = params.get("cursor");
        // (P5-SKLIB-2, C-1128) the library is browsed as the organization (required for an instance library)
        orgs.add(params.get("org"));
        cursors.push(cursor);
        return cursor
          ? json(200, { commit_sha: SHA_B, truncated: false, next_cursor: null, items: mk(50, 5) })
          : json(200, { commit_sha: SHA_B, truncated: false, next_cursor: "c2", items: mk(0, 50) });
      },
    }));
    wrap(<SkillsEditor scope={{ kind: "organization", id: "o1" }} orgId="o1" />);
    fireEvent.click(await screen.findByRole("button", { name: "Add from library…" }));
    const list = await screen.findByRole("list", { name: "Library skills" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(50);
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(55));
    expect(cursors).toEqual([null, "c2"]);
    expect([...orgs]).toEqual(["o1"]);
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
  });

  it("disables Add when the tier's library budget is full, with the numbers", async () => {
    const full = Array.from({ length: 25 }, (_, i) => skill({ id: `s${i}`, name: `lib-${i}`, tier: "agent", source: source({ path: `lib-${i}` }) as Skill["source"] }));
    mockFetch({
      "GET /v1/agents/a1/skills": () => json(200, { items: full, next_cursor: null }),
      "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [{ id: "l1", label: "Team skills", status: "active" }] }),
      "GET /v1/skill-libraries/l1/skills": () => json(200, catalogue),
      "GET /v1/skill-libraries/l1/skills/release-notes": () => json(200, content),
      "GET /v1/agents/a1/skill-library-updates": () => json(200, { items: [] }),
    });
    wrap(<SkillsEditor scope={{ kind: "agent", id: "a1" }} orgId="o1" />);
    fireEvent.click(await screen.findByRole("button", { name: "Add from library…" }));
    // (a heavier render than the other tests: allow for a loaded machine)
    fireEvent.click(await screen.findByRole("button", { name: /^release-notes/ }));
    expect(await screen.findByText(/26 of 25 skills/, undefined)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Add" }) as HTMLButtonElement).disabled).toBe(true);
  });
});

// ---- the line diff ----------------------------------------------------------------------------

describe("diffLines", () => {
  it("keeps every line and marks additions and removals", () => {
    expect(diffLines("a\nb\nc", "a\nB\nc\nd")).toEqual([
      { kind: "same", text: "a" },
      { kind: "del", text: "b" },
      { kind: "add", text: "B" },
      { kind: "same", text: "c" },
      { kind: "add", text: "d" },
    ]);
    expect(diffLines("", "x")).toEqual([{ kind: "add", text: "x" }]);
    expect(diffLines("x", "")).toEqual([{ kind: "del", text: "x" }]);
    const big = Array.from({ length: 5000 }, (_, i) => `line ${i}`).join("\n");
    expect(diffLines(big, big).length).toBe(5000);
  });
});
