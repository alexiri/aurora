import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AdminSkillsPage } from "@/components/admin/admin-topics";
import { SkillLibrariesTab } from "@/components/skills/libraries-page";
import { OrgSkills } from "@/components/skills/org-skills";
import { SkillsList } from "@/components/skills/skills-list";
import { json, type Handler } from "./support/api";
import { wrap } from "./support/render";

// P5-SKLIB-3 (D-450, C-1129): the organization's Libraries page (two sections), the two-step Add dialog, Edit, the
// instance's Skill libraries page, the statuses in words, Add from library's two groups and the update review of a
// skill from an instance library.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme" }),
  useSearchParams: () => new URLSearchParams(),
}));

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
const org = { id: "o1", slug: "acme", name: "Acme", my_role: "owner" };
const orgRoutes = {
  "GET /v1/resolve": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" } }),
  "GET /v1/orgs/o1": () => json(200, org),
};

const library = (over: Record<string, unknown> = {}) => ({
  id: "l1", scope: "org", org_id: "o1", label: "Team skills", status: "active", url: "https://git.example.org/acme/agent-skills.git",
  repo: "git.example.org/acme/agent-skills", ref: null, path: "skills", token_set: false, token_updated_at: null,
  last_head_sha: SHA_B, last_refresh_at: T, last_error: null,
  skill_counts: { agent: 1, workspace: 2, org: 3 }, pending_updates: 2, created_at: T, ...over,
});
const shared = (over: Record<string, unknown> = {}) =>
  library({ id: "l9", scope: "instance", org_id: null, label: "Company skills", url: "https://git.example.org/it/skills.git", repo: "git.example.org/it/skills", path: "", token_set: true, skill_counts: { agent: 0, workspace: 0, org: 1 }, pending_updates: 0, ...over });

const found = (over: Record<string, unknown> = {}) => ({
  url: "https://github.com/acme/skills.git", repo: "github.com/acme/skills", ref: "main", resolved: { kind: "branch", name: "main", default: false },
  commit_sha: SHA_B, path: "skills", skills: { found: 3, valid: 2, names: ["code-review", "release-notes"] }, truncated: false, suggested_label: "skills", ...over,
});

/** The card of the library whose heading is exactly `label`. */
const card = (list: HTMLElement, label: string) => within(list).getByRole("heading", { name: label }).closest("li") as HTMLElement;
/** Radix opens a dropdown on keydown Enter (jsdom has no pointer capture). */
const openMenu = (label: string) => fireEvent.keyDown(screen.getByRole("button", { name: `More actions for ${label}` }), { key: "Enter" });
const LibrariesTab = () => <SkillLibrariesTab slug="acme" orgId="o1" orgName="Acme" />;
const OrgList = () => <SkillsList page={{ kind: "organization", orgId: "o1", label: "Acme" }} />;
/** (P5-SKILLS-3) Add skill › Add from a library, once the libraries are read. */
const fromLibrary = async () => {
  await waitFor(() => {
    fireEvent.keyDown(screen.getByRole("button", { name: "Add skill" }), { key: "Enter" });
    screen.getByRole("menuitem", { name: "Add from a library" });
  });
  fireEvent.click(screen.getByRole("menuitem", { name: "Add from a library" }));
};
/** The words of a card's facts line, in order. */
const facts = (c: HTMLElement) => within(within(c).getByRole("list")).getAllByRole("listitem").map((i) => i.textContent);

// ---- the organization's page ---------------------------------------------------------------------

describe("organization Libraries tab", () => {
  const withLibs = (items: unknown[], extra: Record<string, Handler> = {}) =>
    mockFetch({
      ...orgRoutes,
      "GET /v1/orgs/o1/skill-libraries": () => json(200, { items }),
      "GET /v1/orgs/o1/skills": () => json(200, { items: [], next_cursor: null }),
      ...extra,
    });

  it("lists the organization's own and the ones shared with every organization as cards, never a table", async () => {
    withLibs([library(), shared()]);
    wrap(<LibrariesTab />);
    const own = await screen.findByRole("list", { name: "Your organization's libraries" });
    expect(screen.getByRole("heading", { name: "Shared with every organization" })).toBeTruthy();
    const theirs = screen.getByRole("list", { name: "Libraries shared with every organization" });
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.getByText("Libraries are git repositories of skills. Add one, then pick the skills Acme should use.")).toBeTruthy();
    expect(within(own).getByRole("heading", { name: "Team skills" })).toBeTruthy();
    expect(within(own).queryByText("Company skills")).toBeNull();
    // read-only: the state and this organization's count, and Browse skills; no menu, no Check now, no Edit
    const c = card(theirs, "Company skills");
    expect(within(c).getByText("1 used by Acme")).toBeTruthy();
    expect(within(c).getByRole("button", { name: "Browse skills of Company skills" })).toBeTruthy();
    for (const name of [/^More actions/, /^Edit/, /^Check now/]) expect(within(c).queryByRole("button", { name })).toBeNull();
  });

  it("hides the shared section when the instance shares nothing, and says what a library is when there are none", async () => {
    withLibs([]);
    wrap(<LibrariesTab />);
    expect(await screen.findByText("No libraries yet")).toBeTruthy();
    expect(screen.getByText(/Add a git repository of skills; people then add the skills they want/)).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Shared with every organization" })).toBeNull();
    expect(screen.getAllByRole("button", { name: "Add a library" })).toHaveLength(2);
    expect(screen.queryByText(/Connect a repository/)).toBeNull();
  });

  it("a card says the branch, when it was checked, how many skills the organization uses and what they are pinned to", async () => {
    const orgSkill = (id: string, lib: string, sha: string) => ({ id, name: id, tier: "org", source: { kind: "library", library_id: lib, commit_sha: sha, detached: false } });
    withLibs(
      [
        library({ ref: "main", pending_updates: 0 }),
        library({ id: "l2", label: "Two commits", ref: null, pending_updates: 0, skills_offered: 17, last_refresh_at: null }),
        library({ id: "l3", label: "Unused", skill_counts: { agent: 0, workspace: 0, org: 0 }, skills_offered: 5, pending_updates: 0 }),
        library({ id: "l4", label: "Unused, no offer count", skill_counts: { agent: 0, workspace: 0, org: 0 }, pending_updates: 0 }),
      ],
      { "GET /v1/orgs/o1/skills": () => json(200, { items: [orgSkill("a", "l1", SHA_A), orgSkill("b", "l1", SHA_A), orgSkill("c", "l2", SHA_A), orgSkill("d", "l2", SHA_B)], next_cursor: null }) },
    );
    wrap(<LibrariesTab />);
    const list = await screen.findByRole("list", { name: "Your organization's libraries" });
    await waitFor(() => expect(within(card(list, "Team skills")).getByText("0123456")).toBeTruthy());
    const a = card(list, "Team skills");
    expect(within(a).getByText("git.example.org/acme/agent-skills")).toBeTruthy();
    expect(facts(a)).toEqual(["Branch main", expect.stringMatching(/^Checked /), "6 used by Acme", "Pinned 0123456"]);
    // the repository is plain text, never a link
    expect(within(a).queryByRole("link", { name: /git\.example\.org/ })).toBeNull();
    expect(within(a).queryByRole("link")).toBeNull();
    expect(facts(card(list, "Two commits"))).toEqual(["Default branch", "Not checked yet", "17 skills · 6 used by Acme", "Pinned at 2 commits"]);
    expect(facts(card(list, "Unused")).slice(2)).toEqual(["5 skills · none used yet"]);
    expect(facts(card(list, "Unused, no offer count")).slice(2)).toEqual(["None used yet"]);
  });

  it("names the branch a library with none set follows, and shortens a branch that is a commit id (FIX-279)", async () => {
    withLibs([
      library({ id: "l1", label: "Default", ref: null, default_branch: "main", pending_updates: 0 }),
      library({ id: "l2", label: "Pinned to a commit", ref: SHA_A, default_branch: null, pending_updates: 0 }),
      library({ id: "l3", label: "Unknown", ref: null, default_branch: null, pending_updates: 0 }),
      library({ id: "l4", label: "Hex-like branch", ref: SHA_A.slice(0, 12), default_branch: null, pending_updates: 0 }),
    ]);
    wrap(<LibrariesTab />);
    const list = await screen.findByRole("list", { name: "Your organization's libraries" });
    expect(facts(card(list, "Default"))[0]).toBe("Branch main");
    expect(facts(card(list, "Pinned to a commit"))[0]).toBe("Branch 0123456");
    expect(facts(card(list, "Unknown"))[0]).toBe("Default branch");
    expect(facts(card(list, "Hex-like branch"))[0]).toBe("Branch 0123456789ab");
  });

  it("shows one state chip per card; an update links to the review on the Skills tab", async () => {
    withLibs([
      library({ id: "a", label: "A", pending_updates: 0 }),
      library({ id: "u", label: "U", pending_updates: 1 }),
      library({ id: "b", label: "B", status: "paused" }),
      library({ id: "c", label: "C", status: "error", last_error: "The branch or tag was not found" }),
      library({ id: "d", label: "D", status: "error", last_error: "git said: The repository needs an access token" }),
      library({ id: "e", label: "E", status: "over_budget" }),
      library({ id: "f", label: "F", status: "error", url: null, repo: null, last_error: "Enter the repository's git address" }),
    ]);
    wrap(<LibrariesTab />);
    const list = await screen.findByRole("list", { name: "Your organization's libraries" });
    expect(within(card(list, "A")).getByText("Up to date")).toBeTruthy();
    const u = card(list, "U");
    expect(within(u).getByText("Update available")).toBeTruthy();
    expect(within(u).queryByText("Up to date")).toBeNull();
    expect(within(u).getByRole("link", { name: /1 pending update of U/ }).getAttribute("href")).toBe("/o/acme/skills");
    // a paused library has no update chip and cannot be checked
    const b = card(list, "B");
    expect(within(b).getByText("Paused")).toBeTruthy();
    expect(within(b).queryByText("Update available")).toBeNull();
    expect((within(b).getByRole("button", { name: "Check now B" }) as HTMLButtonElement).disabled).toBe(true);
    const c = card(list, "C");
    expect(within(c).getByText("Needs attention")).toBeTruthy();
    expect(within(c).getByText("The branch or tag was not found")).toBeTruthy();
    const d = card(list, "D");
    expect(within(d).getByText("Needs attention")).toBeTruthy();
    expect(within(d).getByText("The repository needs an access token")).toBeTruthy();
    expect(within(card(list, "E")).getByText("Over budget")).toBeTruthy();
    // a migrated library without an address says what to enter and offers Set address instead of Check now
    const f = card(list, "F");
    expect(within(f).getByText("Enter the repository's git address")).toBeTruthy();
    expect(within(f).getByRole("button", { name: "Set address of F" })).toBeTruthy();
    expect(within(f).queryByRole("button", { name: /^Check now/ })).toBeNull();
  });

  it("check now, pause and delete call the API; delete says how many skills keep their content", async () => {
    const calls: string[] = [];
    withLibs([library()], {
      "POST /v1/skill-libraries/l1/refresh": () => {
        calls.push("refresh");
        return json(202, null);
      },
      "PATCH /v1/skill-libraries/l1": async (r) => {
        calls.push(`patch ${JSON.stringify(await r.json())}`);
        return json(200, library({ status: "paused" }));
      },
      "DELETE /v1/skill-libraries/l1": () => {
        calls.push("delete");
        return json(204, null);
      },
    });
    wrap(<LibrariesTab />);
    fireEvent.click(await screen.findByRole("button", { name: "Check now Team skills" }));
    openMenu("Team skills");
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["Browse skills", "Pause", "Delete…"]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Pause" }));
    await waitFor(() => expect(calls).toHaveLength(2));
    openMenu("Team skills");
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete…" }));
    expect(await screen.findByText("6 skills keep their content and become editable skills.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Delete library" }));
    await waitFor(() => expect(calls).toEqual(["refresh", 'patch {"status":"paused"}', "delete"]));
  });

  it("a paused library offers Resume, and cannot be browsed", async () => {
    withLibs([library({ status: "paused" })]);
    wrap(<LibrariesTab />);
    await screen.findByRole("button", { name: "More actions for Team skills" });
    openMenu("Team skills");
    expect(screen.getByRole("menuitem", { name: "Resume" })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: "Browse skills" }).getAttribute("aria-disabled")).toBe("true");
  });

  it("Browse opens the shared library in read mode: its skills and the preview, no Add", async () => {
    withLibs([shared()], {
      "GET /v1/skill-libraries/l9/skills": (r) => {
        expect(new URL(r.url, "http://localhost").searchParams.get("org")).toBe("o1");
        return json(200, { commit_sha: SHA_B, truncated: false, next_cursor: null, items: [{ path: "triage", name: "triage", description: "Sort issues", valid: true, reason: null, size: 100, files: 1, added: [] }] });
      },
      "GET /v1/skill-libraries/l9/skills/triage": () =>
        json(200, { commit_sha: SHA_B, path: "triage", name: "triage", valid: true, reason: null, description: "Sort issues", body: "Sort them.\n", files: [], size: 40, content_sha256: "a".repeat(64) }),
    });
    wrap(<LibrariesTab />);
    fireEvent.click(await screen.findByRole("button", { name: "Browse skills of Company skills" }));
    const dlg = await screen.findByRole("dialog", { name: "Browse a library" });
    fireEvent.click(await within(dlg).findByRole("button", { name: /^triage/ }));
    expect(await within(dlg).findByLabelText("Preview of what will be stored")).toBeTruthy();
    // looking only: nothing to add, and no library to add from here
    expect(within(dlg).queryByRole("button", { name: /^Add/ })).toBeNull();
    // the header's X and the footer's Close
    expect(within(dlg).getAllByRole("button", { name: "Close" })).toHaveLength(2);
  });

  it("is a calm empty state when the feature is switched off", async () => {
    mockFetch({ ...orgRoutes, "GET /v1/orgs/o1/skill-libraries": () => json(404, { error: { code: "skill_libraries_disabled", message: "off" } }) });
    wrap(<LibrariesTab />);
    expect(await screen.findByText("Skill libraries are turned off on this instance")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Add a library" })).toBeNull();
  });
});

// ---- Add a library -----------------------------------------------------------------------------

describe("Add a library", () => {
  const open = async (extra: Record<string, Handler> = {}) => {
    mockFetch({ ...orgRoutes, "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [] }), ...extra });
    wrap(<LibrariesTab />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Add a library" }))[0]);
    return screen.findByRole("dialog", { name: "Add a library" });
  };
  const type = (label: string, value: string) => fireEvent.change(screen.getByLabelText(label), { target: { value } });

  it("step 1 asks where it is; the token fold is closed and nothing private is sent unless filled", async () => {
    let body: Record<string, unknown> = {};
    const dlg = await open({
      "POST /v1/orgs/o1/skill-libraries/check": async (r) => {
        body = (await r.json()) as Record<string, unknown>;
        return json(200, found());
      },
    });
    expect(within(dlg).getByText(/The address you clone, or the repository's page/)).toBeTruthy();
    expect(within(dlg).getByText("Default: the repository's default branch")).toBeTruthy();
    expect(within(dlg).getByText("Default: the repository root", { exact: false })).toBeTruthy();
    const fold = within(dlg).getByText("This repository is private").closest("details") as HTMLDetailsElement;
    expect(fold.open).toBe(false);
    expect((within(dlg).getByLabelText("Access token") as HTMLInputElement).type, "the token is a secret field").toBe("password");
    expect(within(dlg).getByText("Use a token that can only read this repository. Aurora only reads.")).toBeTruthy();
    expect(within(dlg).getByText("Most hosts accept any name. Bitbucket: x-token-auth")).toBeTruthy();
    // the address is required
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    expect(await within(dlg).findByText("Enter the repository's address.")).toBeTruthy();
    type("Repository address", "https://github.com/acme/skills/tree/main/skills");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    await within(dlg).findByTestId("library-found");
    expect(body).toEqual({ url: "https://github.com/acme/skills/tree/main/skills" });
  });

  it("step 2 shows what Aurora found, as plain text, with the label prefilled; Add library saves what was found", async () => {
    let created: Record<string, unknown> = {};
    const dlg = await open({
      "POST /v1/orgs/o1/skill-libraries/check": () =>
        json(200, found({ skills: { found: 3, valid: 2, names: ["code-review", "<img src=x onerror=alert(1)>"] }, truncated: true })),
      "POST /v1/orgs/o1/skill-libraries": async (r) => {
        created = (await r.json()) as Record<string, unknown>;
        return json(201, library());
      },
    });
    type("Repository address", "https://github.com/acme/skills/tree/main/skills");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    const f = await within(dlg).findByTestId("library-found");
    expect(within(f).getByText("github.com/acme/skills")).toBeTruthy();
    expect(within(f).getByText("main")).toBeTruthy();
    expect(within(f).getByText("89abcde")).toBeTruthy();
    // a page URL filled branch and folder, and the step says so
    expect(within(dlg).getByText("Branch and folder were taken from the address.")).toBeTruthy();
    expect(within(dlg).getByText("2 skills can be added")).toBeTruthy();
    expect(within(dlg).getByText("1 can't be added (open it after adding to see why)")).toBeTruthy();
    const names = within(dlg).getByRole("list", { name: "Skills found" });
    expect(within(names).getByText("<img src=x onerror=alert(1)>")).toBeTruthy();
    expect(dlg.querySelector("img")).toBeNull();
    expect(within(dlg).getByText(/more skill folders than Aurora lists/)).toBeTruthy();
    expect((within(dlg).getByLabelText("Label") as HTMLInputElement).value).toBe("skills");
    fireEvent.change(within(dlg).getByLabelText("Label"), { target: { value: "Acme skills" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Add library" }));
    await waitFor(() => expect(created).toEqual({ label: "Acme skills", url: "https://github.com/acme/skills.git", ref: "main", path: "skills" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add a library" })).toBeNull());
  });

  it("the default branch and a repository root are worded, and no note claims they came from the address", async () => {
    const dlg = await open({
      "POST /v1/orgs/o1/skill-libraries/check": () => json(200, found({ ref: null, path: "", resolved: { kind: "branch", name: "main", default: true }, skills: { found: 1, valid: 1, names: ["one"] } })),
    });
    type("Repository address", "https://git.example.org/team/skills");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    const f = await within(dlg).findByTestId("library-found");
    expect(within(f).getByText("main (default branch)")).toBeTruthy();
    expect(within(f).getByText("Repository root")).toBeTruthy();
    expect(within(dlg).getByText("1 skill can be added")).toBeTruthy();
    expect(within(dlg).queryByText(/taken from the address/)).toBeNull();
    expect(within(dlg).queryByText(/can't be added/)).toBeNull();
  });

  it("Back keeps every field, the filled token opens its fold, and the token is sent with the check and the add", async () => {
    const checks: Record<string, unknown>[] = [];
    let created: Record<string, unknown> = {};
    const dlg = await open({
      "POST /v1/orgs/o1/skill-libraries/check": async (r) => {
        checks.push((await r.json()) as Record<string, unknown>);
        return json(200, found({ ref: null, path: "" }));
      },
      "POST /v1/orgs/o1/skill-libraries": async (r) => {
        created = (await r.json()) as Record<string, unknown>;
        return json(201, library());
      },
    });
    type("Repository address", "https://bitbucket.org/acme/skills");
    type("Branch or tag", "release");
    type("Folder", "agents");
    type("Access token", "s3cret");
    type("User name", "x-token-auth");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    await within(dlg).findByTestId("library-found");
    fireEvent.click(within(dlg).getByRole("button", { name: "Back" }));
    expect((within(dlg).getByLabelText("Repository address") as HTMLInputElement).value).toBe("https://bitbucket.org/acme/skills");
    expect((within(dlg).getByLabelText("Branch or tag") as HTMLInputElement).value).toBe("release");
    expect((within(dlg).getByLabelText("Folder") as HTMLInputElement).value).toBe("agents");
    expect((within(dlg).getByLabelText("Access token") as HTMLInputElement).value).toBe("s3cret");
    expect((within(dlg).getByText("This repository is private").closest("details") as HTMLDetailsElement).open).toBe(true);
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    await within(dlg).findByTestId("library-found");
    fireEvent.click(within(dlg).getByRole("button", { name: "Add library" }));
    await waitFor(() => expect(created).toMatchObject({ url: "https://github.com/acme/skills.git", token: "s3cret", token_username: "x-token-auth" }));
    expect(checks[0]).toEqual({ url: "https://bitbucket.org/acme/skills", ref: "release", path: "agents", token: "s3cret", token_username: "x-token-auth" });
    expect(checks).toHaveLength(2);
  });

  it("a user name without a token is not sent", async () => {
    let body: Record<string, unknown> = {};
    const dlg = await open({
      "POST /v1/orgs/o1/skill-libraries/check": async (r) => {
        body = (await r.json()) as Record<string, unknown>;
        return json(200, found());
      },
    });
    type("Repository address", "https://git.example.org/team/skills");
    type("User name", "someone");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    await within(dlg).findByTestId("library-found");
    expect(body).toEqual({ url: "https://git.example.org/team/skills" });
  });

  it.each([
    ["not_https", "Use the https address; it starts with https://."],
    ["ssh_url", "Use the https address; Aurora doesn't use SSH."],
    ["credentials_in_url", "Take the user name and password out of the address. For a private repository use the access token below."],
    ["query_or_fragment", "Take everything after ? or # out of the address."],
    ["not_a_repository_path", "That address doesn't lead to a repository. Use the repository's address or its page."],
    ["too_long", "That address is too long. Use the repository's own address."],
  ])("shows a refused address (%s) on the address field in words", async (reason, sentence) => {
    const dlg = await open({
      "POST /v1/orgs/o1/skill-libraries/check": () => json(422, { error: { code: "library_url_invalid", message: "bad", details: { reason } } }),
    });
    type("Repository address", "git@github.com:acme/skills.git");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    const field = await within(dlg).findByText(sentence);
    expect(field.id).toBe("lib-url-error");
    expect((within(dlg).getByLabelText("Repository address") as HTMLInputElement).getAttribute("aria-invalid")).toBe("true");
    // still on step 1, nothing found
    expect(within(dlg).queryByTestId("library-found")).toBeNull();
  });

  it("a repository that cannot be read is shown above the fields, with the reader's reason", async () => {
    const dlg = await open({
      "POST /v1/orgs/o1/skill-libraries/check": () => json(422, { error: { code: "library_unreachable", message: "The repository could not be read: The repository was not found, or the token cannot read it." } }),
    });
    type("Repository address", "https://git.example.org/team/skills");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    const alert = await within(dlg).findByRole("alert");
    expect(alert.textContent).toContain("The repository was not found, or the token cannot read it.");
    expect(within(dlg).getByLabelText("Repository address")).toBeTruthy();
    expect(alert.compareDocumentPosition(within(dlg).getByLabelText("Repository address")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("too many checks says when to try again", async () => {
    const dlg = await open({ "POST /v1/orgs/o1/skill-libraries/check": () => json(429, { error: { code: "rate_limited", message: "slow down" } }) });
    type("Repository address", "https://git.example.org/team/skills");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    expect(await within(dlg).findByText("Aurora checks ten repositories a minute. Try again in a minute.")).toBeTruthy();
  });

  it("a failed add stays on step 2 with the reason", async () => {
    const dlg = await open({
      "POST /v1/orgs/o1/skill-libraries/check": () => json(200, found()),
      "POST /v1/orgs/o1/skill-libraries": () => json(409, { error: { code: "library_exists", message: "exists" } }),
    });
    type("Repository address", "https://git.example.org/team/skills");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    await within(dlg).findByTestId("library-found");
    fireEvent.click(within(dlg).getByRole("button", { name: "Add library" }));
    expect(await within(dlg).findByText(/already has a library with this label, or with this repository/)).toBeTruthy();
    expect(within(dlg).getByTestId("library-found")).toBeTruthy();
  });

  it("closing and opening again starts empty", async () => {
    const dlg = await open();
    type("Repository address", "https://git.example.org/team/skills");
    fireEvent.click(within(dlg).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add a library" })).toBeNull());
    fireEvent.click(screen.getAllByRole("button", { name: "Add a library" })[0]);
    expect(((await screen.findByLabelText("Repository address")) as HTMLInputElement).value).toBe("");
  });
});

// ---- Edit ----------------------------------------------------------------------------------------

describe("Edit a library", () => {
  const edit = async (lib: Record<string, unknown>, extra: Record<string, Handler> = {}) => {
    mockFetch({ ...orgRoutes, "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [lib] }), ...extra });
    wrap(<LibrariesTab />);
    fireEvent.click(await screen.findByRole("button", { name: `Edit ${lib.label as string}` }));
    return screen.findByRole("dialog", { name: `Edit ${lib.label as string}` });
  };
  const patched = () => {
    const bodies: Record<string, unknown>[] = [];
    return { bodies, handler: (async (r: Request) => {
      bodies.push((await r.json()) as Record<string, unknown>);
      return json(200, library());
    }) as Handler };
  };

  it("changes label, branch or tag and folder; the address is shown and fixed", async () => {
    const p = patched();
    const dlg = await edit(library(), { "PATCH /v1/skill-libraries/l1": p.handler });
    expect(within(dlg).getByTestId("library-address").textContent).toBe("https://git.example.org/acme/agent-skills.git");
    expect(within(dlg).getByText("To use another repository, add a new library.")).toBeTruthy();
    expect(within(dlg).queryByLabelText("Set address")).toBeNull();
    fireEvent.change(within(dlg).getByLabelText("Label"), { target: { value: "Renamed" } });
    fireEvent.change(within(dlg).getByLabelText("Branch or tag"), { target: { value: "v2" } });
    fireEvent.change(within(dlg).getByLabelText("Folder"), { target: { value: "" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(p.bodies).toEqual([{ label: "Renamed", ref: "v2", path: "" }]));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: /^Edit/ })).toBeNull());
  });

  it("with a token saved: says since when; Replace reveals a secret field and sends it; Remove asks first", async () => {
    const p = patched();
    const dlg = await edit(library({ token_set: true, token_updated_at: "2026-01-02T00:00:00Z" }), { "PATCH /v1/skill-libraries/l1": p.handler });
    expect(within(dlg).getByText(/A token is saved \(since .*2026.*\)\./)).toBeTruthy();
    expect(within(dlg).queryByLabelText("New access token")).toBeNull();
    fireEvent.click(within(dlg).getByRole("button", { name: "Replace" }));
    const token = within(dlg).getByLabelText("New access token") as HTMLInputElement;
    expect(token.type).toBe("password");
    fireEvent.change(token, { target: { value: "newtoken" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(p.bodies).toEqual([{ token: "newtoken" }]));
  });

  it("Remove confirms, then removes the token", async () => {
    const p = patched();
    const dlg = await edit(library({ token_set: true, token_updated_at: T }), { "PATCH /v1/skill-libraries/l1": p.handler });
    fireEvent.click(within(dlg).getByRole("button", { name: "Remove" }));
    const confirm = await screen.findByRole("dialog", { name: "Remove the access token?" });
    expect(p.bodies).toEqual([]);
    fireEvent.click(within(confirm).getByRole("button", { name: "Remove token" }));
    await waitFor(() => expect(p.bodies).toEqual([{ token: null }]));
  });

  it("with no token: says so and offers Add a token", async () => {
    const p = patched();
    const dlg = await edit(library(), { "PATCH /v1/skill-libraries/l1": p.handler });
    expect(within(dlg).getByText("No token.")).toBeTruthy();
    expect(within(dlg).queryByRole("button", { name: "Remove" })).toBeNull();
    fireEvent.click(within(dlg).getByRole("button", { name: "Add a token" }));
    fireEvent.change(within(dlg).getByLabelText("New access token"), { target: { value: "tok" } });
    fireEvent.change(within(dlg).getByLabelText("User name"), { target: { value: "x-token-auth" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(p.bodies).toEqual([{ token: "tok", token_username: "x-token-auth" }]));
  });

  it("a migrated library without an address shows Set address and sends the one PATCH of url", async () => {
    const p = patched();
    mockFetch({
      ...orgRoutes,
      "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [library({ label: "Old", url: null, repo: null, status: "error", last_error: "Enter the repository's git address" })] }),
      "PATCH /v1/skill-libraries/l1": p.handler,
    });
    wrap(<LibrariesTab />);
    fireEvent.click(await screen.findByRole("button", { name: "Set address of Old" }));
    const dlg = await screen.findByRole("dialog", { name: "Edit Old" });
    expect(within(dlg).queryByTestId("library-address")).toBeNull();
    fireEvent.change(within(dlg).getByLabelText("Set address"), { target: { value: "https://git.example.org/acme/old" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(p.bodies).toEqual([{ url: "https://git.example.org/acme/old" }]));
  });

  it("a failed re-read keeps the dialog open with the reason", async () => {
    const dlg = await edit(library(), {
      "PATCH /v1/skill-libraries/l1": () => json(422, { error: { code: "library_unreachable", message: "The repository could not be read: The branch or tag was not found." } }),
    });
    fireEvent.change(within(dlg).getByLabelText("Branch or tag"), { target: { value: "nope" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Save" }));
    expect(await within(dlg).findByText(/The branch or tag was not found/)).toBeTruthy();
    expect(screen.getByRole("dialog", { name: "Edit Team skills" })).toBeTruthy();
  });
});

// ---- Manage <org> › Skills -----------------------------------------------------------------------

describe("Manage <org> › Skills", () => {
  const routes = (role = "owner") => ({
    ...orgRoutes,
    "GET /v1/orgs/o1": () => json(200, { ...org, my_role: role }),
    "GET /v1/orgs/o1/skills": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [] }),
    "GET /v1/orgs/o1/skill-library-updates": () => json(200, { items: [] }),
  });

  it("has the page's heading and lead, and the tabs Skills and Libraries, with Skills first", async () => {
    mockFetch(routes());
    wrap(<OrgSkills />);
    expect(await screen.findByRole("heading", { level: 1, name: "Skills" })).toBeTruthy();
    expect(screen.getByText("What every agent in Acme can do, and where it comes from.")).toBeTruthy();
    expect(screen.getAllByRole("tab").map((t) => [t.textContent, t.getAttribute("aria-selected")])).toEqual([["Skills", "true"], ["Libraries", "false"]]);
    expect(await screen.findByText("No skills yet")).toBeTruthy();
    // (P5-SKILLS-3) the header's Add skill is a menu; writing a new one opens the editor in the list's place
    expect(screen.queryByRole("button", { name: "New skill" })).toBeNull();
    fireEvent.keyDown(screen.getByRole("button", { name: "Add skill" }), { key: "Enter" });
    fireEvent.click(screen.getByRole("menuitem", { name: "Write a new skill" }));
    expect(screen.getByLabelText("When should agents use it?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(screen.queryByLabelText("When should agents use it?")).toBeNull());
    // with no library yet, the way to add one leads to the Libraries tab
    expect((await screen.findByRole("link", { name: "Add a library" })).getAttribute("href")).toBe("/o/acme/skills?tab=libraries");
    fireEvent.click(screen.getByRole("tab", { name: "Libraries" }));
    expect(await screen.findByText("No libraries yet")).toBeTruthy();
    // Add skill belongs to the Skills tab (the router's search parameters do not follow a tab click)
    expect(screen.queryByRole("button", { name: "Add skill" })).toBeNull();
    fireEvent.click(screen.getByRole("tab", { name: "Skills" }));
    expect(screen.getByRole("button", { name: "Add skill" })).toBeTruthy();
  });

  it("is not for members", async () => {
    mockFetch(routes("member"));
    wrap(<OrgSkills />);
    await waitFor(() => expect(screen.queryByRole("tab", { name: "Libraries" })).toBeNull());
    expect(screen.queryByRole("button", { name: "Add a library" })).toBeNull();
  });
});

// ---- Administration › Skills ---------------------------------------------------------------------

describe("Administration › Skills", () => {
  const admin = (extra: Record<string, Handler> = {}) =>
    mockFetch({ "GET /v1/me": () => json(200, { user: { id: "u1", is_instance_admin: true, display_name: "Ada", email: "a@x" } }), ...extra });

  it("lists the instance's libraries as cards with how many organizations use each", async () => {
    admin({ "GET /v1/admin/skill-libraries": () => json(200, { items: [shared({ orgs_using: 3 }), shared({ id: "l8", label: "Unused", orgs_using: 0 })], next_cursor: null }) });
    wrap(<AdminSkillsPage />);
    const list = await screen.findByRole("list", { name: "Skill libraries of this instance" });
    expect(screen.getByRole("heading", { level: 1, name: "Skills" })).toBeTruthy();
    expect(screen.queryByRole("table")).toBeNull();
    const c = card(list, "Company skills");
    expect(within(c).getByText("used by 3 organizations", { exact: false })).toBeTruthy();
    expect(within(c).getByRole("button", { name: "Edit Company skills" })).toBeTruthy();
    expect(within(card(list, "Unused")).getByText("None used yet")).toBeTruthy();
    expect(screen.getByText(/Libraries here are offered to every organization/)).toBeTruthy();
    // browsing a library is an organization's act: the instance's own page has no Browse skills
    openMenu("Company skills");
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["Pause", "Delete…"]);
  });

  it("says there are none and adds through the admin routes", async () => {
    let created: Record<string, unknown> = {};
    admin({
      "GET /v1/admin/skill-libraries": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/admin/skill-libraries/check": () => json(200, found({ ref: null, path: "" })),
      "POST /v1/admin/skill-libraries": async (r) => {
        created = (await r.json()) as Record<string, unknown>;
        return json(201, shared());
      },
    });
    wrap(<AdminSkillsPage />);
    expect(await screen.findByText("No libraries yet")).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { name: "Add a library" })[0]);
    const dlg = await screen.findByRole("dialog", { name: "Add a library" });
    fireEvent.change(within(dlg).getByLabelText("Repository address"), { target: { value: "https://github.com/acme/skills" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    await within(dlg).findByTestId("library-found");
    fireEvent.click(within(dlg).getByRole("button", { name: "Add library" }));
    await waitFor(() => expect(created).toEqual({ label: "skills", url: "https://github.com/acme/skills.git" }));
  });

  it("someone who is not an instance admin gets the admin guard's answer", async () => {
    mockFetch({ "GET /v1/me": () => json(200, { user: { id: "u2", is_instance_admin: false, display_name: "Bo", email: "b@x" } }) });
    wrap(<AdminSkillsPage />);
    await waitFor(() => expect(screen.queryByRole("button", { name: "Add a library" })).toBeNull());
    expect(screen.queryByRole("list", { name: "Skill libraries of this instance" })).toBeNull();
  });
});

// ---- Add from library: two groups, and the update review of an instance library's skill -------------

describe("Add from library with shared libraries", () => {
  const routes = (items: unknown[], extra: Record<string, Handler> = {}) => ({
    "GET /v1/orgs/o1/skills": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/orgs/o1/skill-libraries": () => json(200, { items }),
    "GET /v1/orgs/o1/skill-library-updates": () => json(200, { items: [] }),
    ...extra,
  });
  const lists = (id: string) => ({ commit_sha: SHA_B, truncated: false, next_cursor: null, items: [{ path: `from-${id}`, name: `from-${id}`, description: "d", valid: true, reason: null, size: 10, files: 1, added: [] }] });

  it("groups the picker: your organization, then shared with every organization, labels as data", async () => {
    const orgs: (string | null)[] = [];
    mockFetch(
      routes(
        [{ id: "l1", label: "Team <b>skills</b>", status: "active", scope: "org" }, { id: "l9", label: "Company skills", status: "active", scope: "instance" }, { id: "l8", label: "Paused one", status: "paused", scope: "instance" }],
        {
          "GET /v1/skill-libraries/l1/skills": () => json(200, lists("l1")),
          "GET /v1/skill-libraries/l9/skills": (r) => {
            orgs.push(new URL(r.url, "http://localhost").searchParams.get("org"));
            return json(200, lists("l9"));
          },
        },
      ),
    );
    wrap(<OrgList />);
    await fromLibrary();
    // (P5-SKILLS-4) the libraries are a list on the left; the narrow screen's select keeps the two groups
    const nav = await screen.findByRole("navigation", { name: "Libraries" });
    expect(within(nav).getAllByRole("button").map((b) => b.textContent)).toEqual(["Team <b>skills</b>", "Company skills"]);
    const shared = within(nav).getByRole("list", { name: "Shared with every organization" });
    expect(within(shared).getAllByRole("button").map((b) => b.textContent)).toEqual(["Company skills"]);
    expect(nav.querySelector("b")).toBeNull();
    fireEvent.click(within(shared).getByRole("button", { name: "Company skills" }));
    expect(await screen.findByRole("button", { name: /^From l9/ })).toBeTruthy();
    // an instance library is browsed as this organization
    expect(orgs).toEqual(["o1"]);
  });

  it("is offered when only the instance has an active library", async () => {
    mockFetch(routes([{ id: "l9", label: "Company skills", status: "active", scope: "instance" }]));
    wrap(<OrgList />);
    await fromLibrary();
    expect(await screen.findByRole("dialog", { name: "Add a skill from a library" })).toBeTruthy();
  });

  it("is not offered when the only library is paused", async () => {
    const fn = mockFetch(routes([{ id: "l9", label: "Company skills", status: "paused", scope: "instance" }]));
    wrap(<OrgList />);
    await screen.findByRole("button", { name: "Add skill" });
    await waitFor(() => expect(fn.mock.calls.some(([r]) => (r as Request).url.includes("/skill-libraries"))).toBe(true));
    fireEvent.keyDown(screen.getByRole("button", { name: "Add skill" }), { key: "Enter" });
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["Write a new skill", "Upload a skill folder"]);
  });

  it("adds a skill from an instance library with the instance library's id", async () => {
    let added: unknown = null;
    mockFetch(
      routes([{ id: "l9", label: "Company skills", status: "active", scope: "instance" }], {
        "GET /v1/skill-libraries/l9/skills": () => json(200, lists("l9")),
        "GET /v1/skill-libraries/l9/skills/from-l9": () =>
          json(200, { commit_sha: SHA_B, path: "from-l9", name: "from-l9", valid: true, reason: null, description: "d", body: "b\n", files: [], size: 20, content_sha256: "a".repeat(64) }),
        "POST /v1/orgs/o1/skills": async (r) => {
          added = await r.json();
          return json(201, { id: "new", name: "from-l9", tier: "org", scope: "organization", org_id: "o1", description: "d", body: "b\n", enabled: true, files: [], total_size: 20, source: null, created_at: T, updated_at: T });
        },
      }),
    );
    wrap(<OrgList />);
    await fromLibrary();
    fireEvent.click(await screen.findByRole("button", { name: /^From l9/ }));
    await screen.findByLabelText("Preview of what will be stored");
    fireEvent.click(screen.getByRole("button", { name: "Add to Acme" }));
    await waitFor(() => expect(added).toEqual({ from_library: { library_id: "l9", path: "from-l9", commit_sha: SHA_B } }));
  });
});

describe("update review of a skill from an instance library", () => {
  const skill = {
    id: "s7", tier: "org", scope: "organization", agent_id: null, workspace_id: null, org_id: "o1", name: "triage", description: "Sort issues", body: "Sort.\n", enabled: true,
    files: [], total_size: 20, overridden_in: null, created_at: T, updated_at: T,
    source: { kind: "library", library_id: "l9", label: "Company skills", repo: "git.example.org/it/skills", path: "triage", commit_sha: SHA_A, pinned_at: T, detached: false, update_pending: true },
  };

  it("shows the badge with the host/path repository and the commit, and Approve applies exactly the reviewed content", async () => {
    let approved: unknown = null;
    mockFetch({
      "GET /v1/orgs/o1/skills": () => json(200, { items: [skill], next_cursor: null }),
      "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [shared()] }),
      "GET /v1/orgs/o1/skill-library-updates": () => json(200, { items: [{ id: "u7", kind: "changed", created_at: T, skill: { id: "s7", name: "triage", tier: "org" }, library: { id: "l9", label: "Company skills" } }] }),
      "GET /v1/skills/s7/library": () =>
        json(200, {
          library: { id: "l9", label: "Company skills", repo: "git.example.org/it/skills", path: "triage" }, commit_sha: SHA_A, pinned_at: T, pinned_by: null, detached: false,
          update: {
            id: "u7", kind: "changed", commit_sha: SHA_B, created_at: T, content_sha256: "c".repeat(64), over_budget: false, reason: null,
            diff: { description: { old: "Sort issues", new: "Sort issues fast", changed: true }, body: { old: "Sort.\n", new: "Sort.\nThen close.\n", changed: true }, files: [] },
          },
        }),
      "POST /v1/skills/s7/library/updates/u7/approve": async (r) => {
        approved = await r.json();
        return json(200, skill);
      },
    });
    wrap(<OrgList />);
    expect(await screen.findByTestId("source-chip")).toHaveTextContent("Library · Company skills");
    fireEvent.click(screen.getByRole("button", { name: "triage" }));
    expect((await screen.findByTestId("skill-lock")).textContent).toContain("From Company skills · pinned to 0123456.");
    fireEvent.click(await screen.findByRole("button", { name: "Review update" }));
    const dlg = await screen.findByRole("dialog");
    expect((await within(dlg).findByLabelText("SKILL.md diff")).textContent).toContain("+ Then close.");
    fireEvent.click(within(dlg).getByRole("button", { name: "Approve" }));
    await waitFor(() => expect(approved).toEqual({ content_sha256: "c".repeat(64) }));
    cleanup();
  });
});
