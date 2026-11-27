import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { NARROW_BROWSER, splitCatalogue } from "@/components/skills/add-from-library";
import { SkillsList } from "@/components/skills/skills-list";
import { err, json } from "./support/api";
import { wrap } from "./support/render";
import { AgentSkills, SHA_A, applied, effective, librarySource, me, openMenu, routes, skill } from "./support/skills";

// P5-SKILLS-4 (skill-5, S-10; D-496, D-498, C-1176): Add a skill from a library: the libraries, only what can be
// added (the rest folded with why), a preview, and Add to the agent, the workspace or the organization.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(),
}));

beforeEach(() => window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=skills"));
afterEach(() => vi.unstubAllGlobals());

const libraries = [
  { id: "l1", label: "Anthropic skills", status: "active", scope: "org", skills_offered: 17 },
  { id: "l2", label: "Acme team recipes", status: "active", scope: "org", skills_offered: 1 },
  { id: "l9", label: "Platform <b>tools</b>", status: "active", scope: "instance", skills_offered: null },
  { id: "l3", label: "Paused one", status: "paused", scope: "org", skills_offered: 4 },
];

const item = (name: string, over: Record<string, unknown> = {}) => ({
  path: name, name, description: `${name} does things.`, valid: true, reason: null, size: 2048, files: 2, added: [], blocked: null, ...over,
});

const mine = skill({ id: "own-pdf", name: "pdf", source: librarySource() });

const catalogue = (over: Record<string, unknown> = {}) => ({
  commit_sha: SHA_A,
  truncated: false,
  next_cursor: null,
  budget: { bytes: 32_768, used: 30_000 },
  items: [
    item("brand-guidelines", { description: "Apply Anthropic’s brand colors." }),
    item("doc-coauthoring", { files: 1, size: 100, added: [{ tier: "agent", id: "someone-elses" }] }),
    // this very skill, already here: listed and marked, though its folder name is taken
    item("pdf", { added: [{ tier: "agent", id: "own-pdf" }], blocked: { reason: "name_taken", message: "This agent already has a skill named pdf." } }),
    item("triage-alerts", { blocked: { reason: "name_taken", message: "This agent already has a skill named triage-alerts." } }),
    item("huge-one", { size: 9000, blocked: { reason: "budget", message: "Too large for the room left for library skills here (30000 of 32768 characters used)." } }),
    item("one-too-many", { size: 10, blocked: { reason: "budget", message: "No more library skills fit here (25 of 25 used)." } }),
    item("Bad_Name", { valid: false, size: 0, description: null, reason: "the directory name is not a valid skill name", reason_code: "bad_name", blocked: { reason: "invalid", message: "the directory name is not a valid skill name" } }),
  ],
  ...over,
});

const BODY = "## Colors\n\n- Dark `#141413`\n\n![pixel](/v1/me)\n\n<picture><source srcset=\"/v1/x\"><img src=\"/v1/y\" alt=\"swatch\"></picture>";
const content = (over: Record<string, unknown> = {}) => ({
  commit_sha: SHA_A, path: "brand-guidelines", name: "brand-guidelines", valid: true, reason: null, description: "Apply Anthropic’s brand colors.",
  body: BODY, size: 2000, content_sha256: "a".repeat(64),
  files: [
    { path: "LICENSE.txt", size: 307, executable: false, binary: false, content: "<b>MIT</b>", content_b64: null },
    { path: "logo.png", size: 100, executable: false, binary: true, content: null, content_b64: "AA==" },
  ],
  ...over,
});

const base = (extra: Parameters<typeof routes>[0] = {}, role: "owner" | "admin" | "member" = "member") => ({
  "GET /v1/me": () => json(200, me(role)),
  "GET /v1/agents/a1/skills/effective": effective([applied(mine)]),
  "GET /v1/agents/a1/skill-library-updates": () => json(200, { items: [] }),
  "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: libraries }),
  "GET /v1/skill-libraries/l1/skills": () => json(200, catalogue()),
  "GET /v1/skill-libraries/l1/skills/brand-guidelines": () => json(200, content()),
  ...extra,
});

async function openDialog() {
  await waitFor(() => {
    openMenu("Add skill");
    screen.getByRole("menuitem", { name: "Add from a library" });
  });
  fireEvent.click(screen.getByRole("menuitem", { name: "Add from a library" }));
  return screen.findByRole("dialog", { name: "Add a skill from a library" });
}

describe("the dialog (skill-5)", () => {
  it("has the libraries on the left with their counts, the shared ones told apart, and says what a version is", async () => {
    routes(base());
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    expect(within(dlg).getByText("Skills are pinned to the version you see here.")).toBeTruthy();
    const nav = within(dlg).getByRole("navigation", { name: "Libraries" });
    expect(within(nav).getAllByRole("button").map((b) => b.textContent)).toEqual(["Anthropic skills17 skills", "Acme team recipes1 skill", "Platform <b>tools</b>"]);
    expect(within(nav).getByRole("button", { name: /Anthropic skills/ })).toHaveAttribute("aria-current", "true");
    expect(within(within(nav).getByRole("list", { name: "Shared with every organization" })).getByText("Platform <b>tools</b>")).toBeTruthy();
    // a label is data
    expect(nav.querySelector("b")).toBeNull();
    expect(nav.textContent).not.toContain("Paused one");
  });

  it("asks the server what can be added to this agent (C-1176)", async () => {
    const calls = routes(base());
    wrap(<AgentSkills />);
    await openDialog();
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/skill-libraries/l1/skills")).toBe(true));
    const query = new URLSearchParams(calls.find((c) => c.path === "/v1/skill-libraries/l1/skills")!.query);
    expect(Object.fromEntries(query)).toEqual({ org: "o1", tier: "agent", agent: "a1" });
  });

  it("lists only what can be added, marks what is here as Added, and folds the rest with why", async () => {
    routes(base());
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    const list = await within(dlg).findByRole("list", { name: "Library skills" });
    const rows = within(list).getAllByRole("button");
    expect(rows.map((r) => r.textContent)).toEqual([
      "Brand guidelinesApply Anthropic’s brand colors.2 files · 2 KB",
      // added to another agent: nothing says Added here
      "Doc coauthoringDoc-coauthoring does things.1 file · 1 KB",
      "pdfPdf does things.2 files · 2 KBAdded",
    ]);
    const fold = within(dlg).getByRole("button", { name: /4 can’t be added/ });
    expect(fold.textContent).toBe("4 can’t be added · Why?");
    expect(fold).toHaveAttribute("aria-expanded", "false");
    expect(within(dlg).queryByRole("list", { name: "Skills that can’t be added" })).toBeNull();
    fireEvent.click(fold);
    const why = within(dlg).getByRole("list", { name: "Skills that can’t be added" });
    expect(within(why).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "Triage alertsA skill named triage-alerts is already on Daneel.",
      "Huge oneIt would exceed Daneel’s 32 KB of library skills.",
      "One too manyNo more library skills fit here (25 of 25 used).",
      "Bad_NameIts folder name can’t be used as a skill name.",
    ]);
    expect(document.body.textContent).not.toMatch(/about 0 characters|Cannot be added/);
  });

  it("opens on the first skill that can be added (FIX-279)", async () => {
    routes(
      base({
        "GET /v1/skill-libraries/l1/skills": () =>
          json(200, catalogue({ items: [item("pdf", { added: [{ tier: "agent", id: "own-pdf" }] }), item("brand-guidelines")] })),
      }),
    );
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    expect(await within(dlg).findByRole("heading", { name: "Brand guidelines" })).toBeTruthy();
  });

  it("…and on nothing when none can be added (FIX-279)", async () => {
    routes(
      base({
        "GET /v1/skill-libraries/l1/skills": () =>
          json(200, catalogue({ items: [item("pdf", { added: [{ tier: "agent", id: "own-pdf" }] })] })),
      }),
    );
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    await within(dlg).findByRole("list", { name: "Library skills" });
    expect(within(dlg).queryByRole("heading", { level: 3 })).toBeNull();
    expect(within(dlg).queryByRole("group", { name: "Preview of what will be stored" })).toBeNull();
    expect(within(dlg).getByRole("button", { name: "Add to Daneel" })).toBeDisabled();
    // what is already here can still be pressed and read
    fireEvent.click(within(dlg).getByRole("button", { name: /^pdf/ }));
    expect(await within(dlg).findByRole("heading", { name: "pdf" })).toBeTruthy();
  });

  it("a skill the server could not judge (size null) is folded with its sentence, shows no size and stays out of the budget (FIX-279)", async () => {
    routes(
      base({
        "GET /v1/skill-libraries/l1/skills": () =>
          json(
            200,
            catalogue({
              items: [
                item("big-one", { valid: false, size: null, description: null, reason: "raw server text", reason_code: "unknown_size", blocked: { reason: "invalid", message: "raw server text" } }),
                item("brand-guidelines"),
              ],
            }),
          ),
      }),
    );
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    expect(await within(dlg).findByRole("heading", { name: "Brand guidelines" })).toBeTruthy();
    fireEvent.click(within(dlg).getByRole("button", { name: /1 can’t be added/ }));
    const why = within(dlg).getByRole("list", { name: "Skills that can’t be added" });
    expect(why.textContent).toBe("Big oneAurora couldn’t read all of it to check its size, so it can’t be added.");
    expect(dlg.textContent).not.toMatch(/0 KB|NaN|raw server text/);
  });

  it("an unjudged skill of a library being looked at shows its files without a size (FIX-279)", () => {
    // Without a target there is no `blocked`: the code alone folds it.
    const { listed, folded } = splitCatalogue(
      [item("big-one", { valid: true, size: null, reason_code: "unknown_size" }) as never],
      undefined,
      null,
    );
    expect(listed).toEqual([]);
    expect(folded.map((f) => f.why)).toEqual(["Aurora couldn’t read all of it to check its size, so it can’t be added."]);
  });

  it("previews the chosen skill in full, fetching nothing for its text", async () => {
    routes(base());
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    // beside the list there is a skill to read from the start
    const preview = await within(dlg).findByRole("group", { name: "Preview of what will be stored" });
    expect(within(dlg).getByRole("heading", { name: "Brand guidelines" })).toBeTruthy();
    // repository text stays as written
    expect(within(preview).getByText("Apply Anthropic’s brand colors.")).toBeTruthy();
    expect(within(preview).getByRole("heading", { name: "Colors" })).toBeTruthy();
    expect(preview.querySelectorAll("img, source, picture")).toHaveLength(0);
    expect(preview.textContent).toContain("image: pixel");
    expect(preview.textContent).toContain("image: swatch");
    const files = within(preview).getByRole("list", { name: "Files" });
    expect(within(files).getAllByRole("button").map((b) => b.textContent)).toEqual([`SKILL.md${(([...BODY].length / 1024)).toFixed(1)} KB`, "LICENSE.txt0.3 KB", "logo.png0.1 KB"]);
    fireEvent.click(within(files).getByRole("button", { name: /LICENSE/ }));
    const view = within(preview).getByRole("region", { name: "File LICENSE.txt" });
    expect(view.textContent).toBe("<b>MIT</b>");
    expect(view.querySelector("b")).toBeNull();
    fireEvent.click(within(files).getByRole("button", { name: /logo/ }));
    expect(within(preview).getByText("This file isn't text, so it isn't shown.")).toBeTruthy();
    expect(preview.textContent).toContain("Pinned to e4bb5d9 when added.");
    expect(within(dlg).getByText("Adds a copy to Daneel. You can turn it off or remove it later.")).toBeTruthy();
  });

  it("Add to Daneel adds the pinned copy, closes, and opens the new skill's panel", async () => {
    const added = skill({ id: "new", name: "brand-guidelines", source: librarySource({ path: "brand-guidelines" }) });
    let done = false;
    const calls = routes(
      base({
        "GET /v1/agents/a1/skills/effective": () => json(200, { items: done ? [applied(mine), applied(added)] : [applied(mine)], shadowed: [] }),
        "POST /v1/agents/a1/skills": () => {
          done = true;
          return json(201, added);
        },
      }),
    );
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    const button = within(dlg).getByRole("button", { name: "Add to Daneel" });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    expect(await screen.findByRole("article", { name: "Brand guidelines" })).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(JSON.parse(calls.find((c) => c.method === "POST")!.body)).toEqual({ from_library: { library_id: "l1", path: "brand-guidelines", commit_sha: SHA_A } });
  });

  it("what is already added can be read but not added again", async () => {
    routes(base({ "GET /v1/skill-libraries/l1/skills/pdf": () => json(200, content({ path: "pdf", name: "pdf", size: 10 })) }));
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    fireEvent.click(await within(dlg).findByRole("button", { name: /^pdf/ }));
    expect(await within(dlg).findByText("Already added to Daneel.")).toBeTruthy();
    await within(dlg).findByRole("group", { name: "Preview of what will be stored" });
    expect(within(dlg).getByRole("button", { name: "Add to Daneel" })).toBeDisabled();
  });

  it.each([
    ["the files are larger than the listing's estimate", { size: 5000 }, "It would exceed Daneel’s 32 KB of library skills."],
    ["the read finds it broken", { valid: false, reason: "a symlink is not allowed (link)", reason_code: "unsupported_entry", size: 0 }, "It holds a link, a submodule or another kind of file a skill can’t hold."],
    ["the read finds it broken and gives a code this page does not know", { valid: false, reason: "something new (x)", reason_code: "made_up", size: 0 }, "This skill can’t be added."],
  ])("Add is off with the reason when %s", async (_when, over, why) => {
    routes(base({ "GET /v1/skill-libraries/l1/skills/brand-guidelines": () => json(200, content(over)) }));
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    const button = await within(dlg).findByRole("button", { name: "Add to Daneel" });
    // (the same sentence also sits in the folded list, hidden)
    await waitFor(() => expect(button).toHaveAccessibleDescription(why));
    expect(button).toBeDisabled();
  });

  it.each([
    [409, "conflict", "A skill with this name already exists in this scope."],
    [422, "library_budget_exceeded", "This agent would exceed its library skill budget (40000 of 32768 characters, 3 of 25 skills)."],
  ])("a %i from the server is said on the preview, the dialog stays and the list is read again", async (status, code, message) => {
    const calls = routes(base({ "POST /v1/agents/a1/skills": () => err(status, code, message) }));
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    const button = within(dlg).getByRole("button", { name: "Add to Daneel" });
    await waitFor(() => expect(button).toBeEnabled());
    const reads = () => calls.filter((c) => c.path === "/v1/skill-libraries/l1/skills").length;
    const before = reads();
    fireEvent.click(button);
    const alert = await within(dlg).findByRole("alert");
    expect(alert.textContent).toContain(message);
    await waitFor(() => expect(reads()).toBeGreaterThan(before));
    expect(screen.getByRole("dialog", { name: "Add a skill from a library" })).toBeTruthy();
  });

  it("searches on the server and starts another library afresh", async () => {
    const calls = routes(base({ "GET /v1/skill-libraries/l2/skills": () => json(200, catalogue({ items: [] })) }));
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    fireEvent.change(within(dlg).getByRole("searchbox", { name: "Search skills" }), { target: { value: "brand" } });
    await waitFor(() => expect(calls.some((c) => new URLSearchParams(c.query).get("q") === "brand")).toBe(true));
    fireEvent.click(within(dlg).getByRole("button", { name: /Acme team recipes/ }));
    expect(await within(dlg).findByText("No skills found")).toBeTruthy();
    expect(within(dlg).getByRole("searchbox", { name: "Search skills" })).toHaveValue("");
    expect(within(dlg).getByRole("button", { name: "Add to Daneel" })).toBeDisabled();
  });

  it("offers Add a library to the organization's admins only, and opens that dialog", async () => {
    routes(base({}, "admin"));
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    fireEvent.click(await within(dlg).findByRole("button", { name: "Add a library" }));
    expect(await screen.findByRole("dialog", { name: "Add a library" })).toBeTruthy();
  });

  it("…and nothing of the kind to a member", async () => {
    routes(base());
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    await within(dlg).findByRole("group", { name: "Preview of what will be stored" });
    expect(within(dlg).queryByRole("button", { name: "Add a library" })).toBeNull();
  });
});

describe("for a workspace and the organization", () => {
  const level = (scope: string, add: string) =>
    base({
      [`GET ${scope}`]: () => json(200, { items: [], next_cursor: null }),
      [`POST ${scope}`]: () => json(201, skill({ id: "new", name: "brand-guidelines", tier: add })),
    });

  it.each([
    ["workspace", { kind: "workspace", wsId: "w1", orgId: "o1", label: "Main", canOwn: true } as const, "/v1/workspaces/w1/skills", { org: "o1", tier: "workspace", workspace: "w1" }],
    ["org", { kind: "organization", orgId: "o1", label: "Acme" } as const, "/v1/orgs/o1/skills", { org: "o1", tier: "organization" }],
  ])("adds to the %s, with its words", async (add, page, path, query) => {
    const calls = routes(level(path, add));
    wrap(<SkillsList page={page} />);
    const dlg = await openDialog();
    expect(within(dlg).getByText(`Adds it to ${page.label}’s skills. You can turn it off or remove it later.`)).toBeTruthy();
    const button = within(dlg).getByRole("button", { name: `Add to ${page.label}` });
    await waitFor(() => expect(button).toBeEnabled());
    expect(Object.fromEntries(new URLSearchParams(calls.find((c) => c.path === "/v1/skill-libraries/l1/skills")!.query))).toEqual(query);
    fireEvent.click(button);
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === path)).toBe(true));
  });
});

describe("on a narrow screen", () => {
  beforeEach(() => {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: query === NARROW_BROWSER, media: query, addEventListener: () => undefined, removeEventListener: () => undefined }));
  });

  it("the libraries are a select above the list, and a skill is its own screen with ‹ Skills", async () => {
    routes(base());
    wrap(<AgentSkills />);
    const dlg = await openDialog();
    const pick = within(dlg).getByLabelText("Library") as HTMLSelectElement;
    expect([...pick.querySelectorAll("optgroup")].map((g) => g.label)).toEqual(["Your organization", "Shared with every organization"]);
    expect(within(dlg).queryByRole("navigation", { name: "Libraries" })).toBeNull();
    // nothing is chosen for the person: the list is the screen
    expect(within(dlg).queryByRole("group", { name: "Preview of what will be stored" })).toBeNull();
    expect(within(dlg).queryByRole("button", { name: "Add to Daneel" })).toBeNull();
    fireEvent.click(await within(dlg).findByRole("button", { name: /^Brand guidelines/ }));
    await within(dlg).findByRole("group", { name: "Preview of what will be stored" });
    expect(within(dlg).queryByRole("list", { name: "Library skills" })).toBeNull();
    expect(within(dlg).getByRole("button", { name: "Add to Daneel" })).toBeTruthy();
    fireEvent.click(within(dlg).getByRole("button", { name: "Skills" }));
    expect(within(dlg).getByRole("list", { name: "Library skills" })).toBeTruthy();
  });
});
