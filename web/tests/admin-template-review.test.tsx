import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { TemplateVersionReview } from "@/components/templates/review-version";
import { err, json, type Handler } from "./support/api";
import { wrap } from "./support/render";

// C-1114: the review of one version — everything in full as data, marked as text from the source at
// the commit, the README with its Preview toggle, what changed against the version people use now
// (FIX-247) shown as each part's diff inside its own section, folded GitLab-style, with a short
// what-changed block on top (FIX-250), Approve bound to the exact content_sha256 the page showed (a
// 409 reloads with the version-changed sentence), and Reject with a confirm.

vi.mock("next/navigation", () => ({
  useParams: () => ({ template_id: "schedule-manager", version: "1.1.0" }),
}));

const SHA_A = "0123456789abcdef0123456789abcdef01234567";
const SHA_B = "89abcdef0123456789abcdef0123456789abcdef";
const SHA_CONTENT = "c".repeat(64);

const content = (over: Record<string, unknown> = {}) => ({
  name: "Schedule manager", summary: "Triage and schedule the team's work",
  readme: "# Schedule manager\n\nWhat it does.\n\n## Adapting it\n",
  effort: null, min_core: null, agent: { name: "Daneel", may_message_first: false },
  instructions: "Triage carefully.",
  inputs: [{ id: "repo", label: "Repository", help: "The repository to watch.", kind: "text", required: true, pattern: null, max_length: null, options: null, default: null }],
  apps: [{ app: "github", required: true, level: "ask", tools: null, why: "Reads the pull requests" }],
  skills: [{ name: "code-review", use_it_when: "A pull request is open", body: "Review carefully.\n", files: [{ path: "checklist.md", size: 10, executable: false, sha256: "e".repeat(64), text: "- tests\n" }] }],
  skill_refs: [{ kind: "library", address: "https://git.example.org/acme/skills.git", ref: "main", path: "skills", skill: "release-notes", required: true, why: "Writes release notes" }],
  helpers: [{ path: "scripts/run.sh", size: 20, executable: true, sha256: "f".repeat(64), text: "#!/bin/sh\n" }],
  memories: [{ key: "people", content: "Ada leads the platform team.", locked: true }],
  automations: [{ kind: "schedule", name: "morning-triage", prompt: "Triage the queue.", cron: "0 9 * * *" }],
  has_avatar: false,
  ...over,
});

const version = (v: string, sha: string, sha256: string, c: Record<string, unknown>, over: Record<string, unknown> = {}) => ({
  template_id: "schedule-manager", version: v, status: v === "1.0.0" ? "approved" : "pending_approval", status_reason: null,
  commit_sha: sha, content_sha256: sha256, archive_sha256: "d".repeat(64), size_bytes: 4096,
  source: { id: "s1", name: "Team templates", repo: "github.com/acme/agent-templates", ref: null, path: "templates" },
  content: c,
  ...over,
});

const detail = version("1.1.0", SHA_B, SHA_CONTENT, content());
const offered = version("1.0.0", SHA_A, "b".repeat(64), content({ instructions: "Triage the queue." }));

const listRoute = (item: Record<string, unknown> = {}) => ({
  "GET /v1/admin/agent-templates": () =>
    json(200, {
      items: [{ template_id: "schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work", source: { id: "s1", name: "Team templates" }, current_version: "1.0.0", offered: true, head_state: "ok", head_version: null, head_problems: null, removed_upstream: false, needs_apps: [], needs_libraries: [], versions: [], ...item }],
    }),
});

function mockFetch(routes: Record<string, Handler>, onWrite?: (key: string, body: unknown) => void) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const key = `${req.method} ${decodeURIComponent(url.pathname)}`;
    const h = routes[key];
    if (h) return h(req);
    if (onWrite && req.method !== "GET") {
      onWrite(key, JSON.parse((await req.text()) || "null"));
      return json(200, detail);
    }
    return json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

function renderReview(routes: Record<string, Handler> = {}, onWrite?: (key: string, body: unknown) => void) {
  mockFetch({
    ...listRoute(),
    "GET /v1/admin/agent-templates/schedule-manager/versions/1.1.0": () => json(200, detail),
    "GET /v1/admin/agent-templates/schedule-manager/versions/1.0.0": () => json(200, offered),
    ...routes,
  }, onWrite);
  wrap(<TemplateVersionReview templateId="schedule-manager" version="1.1.0" />);
}

/** Approve is enabled once the comparison is ready (FIX-271). */
async function clickApprove() {
  const button = (await screen.findByRole("button", { name: "Approve" })) as HTMLButtonElement;
  await waitFor(() => expect(button.disabled).toBe(false));
  fireEvent.click(button);
}

describe("Agent template review (C-1114)", () => {
  afterEach(cleanup);

  it("shows About this template as source text, with Preview rendering it as people will see it", async () => {
    renderReview();
    const section = (await screen.findByRole("region", { name: /About this template/ })) as unknown as HTMLElement;
    const src = within(section).getByLabelText("README.md source text");
    expect(src.textContent).toContain("# Schedule manager");
    fireEvent.click(within(section).getByRole("button", { name: "Preview" }));
    const md = within(section).getByTestId("template-readme");
    expect(md.querySelector("h1")).toBeNull();
    expect(within(md).getByRole("heading", { level: 3, name: "Schedule manager" })).toBeTruthy();
    expect(within(md).getByRole("heading", { level: 4, name: "Adapting it" })).toBeTruthy();
    // and back to the source text
    fireEvent.click(within(section).getByRole("button", { name: "Source text" }));
    expect(within(section).getByLabelText("README.md source text")).toBeTruthy();
  });

  it("shows every part in full, marked as text from the source at the commit", async () => {
    renderReview();
    await screen.findByRole("heading", { name: "Schedule manager 1.1.0" });
    expect(screen.getByText(/Text from/).textContent).toContain("Team templates");
    expect(screen.getByText(/The package matches the repository at/).textContent).toContain("89abcde");
    // instructions, the skill with its file, helper files (executable in words)
    expect(screen.getByRole("region", { name: "Instructions" }).textContent).toContain("Triage carefully.");
    expect(screen.getByLabelText("Skill code-review body").textContent).toContain("Review carefully.");
    expect(screen.getByLabelText("File checklist.md of skill code-review").textContent).toContain("- tests");
    const helpers = screen.getByRole("region", { name: "Helper files" });
    expect(helpers.textContent).toContain("executable");
    expect(helpers.textContent).toContain("#!/bin/sh");
    // skill references with the C-1114 sentence
    expect(screen.getByTestId("template-skill-ref").textContent).toContain("Comes from that library as it is when an agent is created; its updates are reviewed in the library.");
    expect(screen.getByTestId("template-skill-ref").textContent).toContain("https://git.example.org/acme/skills.git · main · skills");
    // memories with the locked wording
    const mem = screen.getByTestId("template-memory");
    expect(mem.textContent).toContain("Locked: Daneel can read it but not change it.");
    expect(mem.textContent).toContain("Ada leads the platform team.");
    // automations with the cron in Technical details; the prompt in words
    expect(screen.getByText("Triage the queue.")).toBeTruthy();
    expect(screen.getByText("0 9 * * *")).toBeTruthy();
    // apps with their level and why; inputs with their label
    expect(screen.getByText("Ask access.")).toBeTruthy();
    expect(screen.getByText("Reads the pull requests")).toBeTruthy();
    expect(screen.getByText("Repository")).toBeTruthy();
  });

  it("shows each part's diff inside its section, the top block short", async () => {
    renderReview();
    await screen.findByText("People keep using 1.0.0 until you approve this one.");
    const block = await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    expect(within(block).getByText("1 change: Instructions.")).toBeTruthy();
    // The block holds no diff of its own.
    expect(within(block).queryByTestId("diff-view")).toBeNull();
    // The diff sits in the Instructions section, not above the content.
    const instructions = screen.getByRole("region", { name: "Instructions" });
    expect(block.compareDocumentPosition(instructions) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(instructions).getByLabelText("Diff of Instructions")).toBeTruthy();
    const diff = within(instructions).getByLabelText("Diff of Instructions");
    expect(diff.textContent).toContain("- Triage the queue.");
    expect(diff.textContent).toContain("+ Triage carefully.");
    // A section nothing touched is collapsed to one line "Unchanged" that expands to the text.
    const about = screen.getByRole("region", { name: /About this template/ });
    const fold = within(about).getByTestId("diff-unchanged");
    expect(fold.textContent).toContain("Unchanged");
    expect(within(fold).getByLabelText("README.md source text").textContent).toContain("# Schedule manager");  });

  // (FIX-289) The diff, with its folds opened, is the current text and the change.
  it("a changed part shows its diff and no Show the current text button", async () => {
    renderReview();
    await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    const instructions = screen.getByRole("region", { name: "Instructions" });
    expect(within(instructions).getByLabelText("Diff of Instructions")).toBeTruthy();
    expect(within(instructions).queryByRole("button", { name: /current text/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /current text/ })).toBeNull();
  });

  it("calls out the changes that widen what the agent can do or reach first, linking to their sections", async () => {
    renderReview({
      "GET /v1/admin/agent-templates/schedule-manager/versions/1.1.0": () =>
        json(200, {
          ...detail,
          content: content({
            apps: [
              { app: "github", required: true, level: "ask", tools: null, why: "Reads the pull requests" },
              { app: "slack", required: false, level: "read", tools: null, why: "Posts to the channel" },
            ],
            automations: [
              { kind: "schedule", name: "morning-triage", prompt: "Triage the queue.", cron: "0 9 * * *" },
              { kind: "webhook", name: "pager", instructions: "Page the on-call.", verification: "signed" },
            ],
          }),
        }),
    });
    const block = await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    expect(within(block).getByText("3 changes: Instructions, a new app, a new webhook.")).toBeTruthy();
    const attention = within(block).getByTestId("diff-attention");
    expect(within(attention).getByText("Look at these first")).toBeTruthy();
    expect(within(attention).getByRole("link", { name: "App slack" }).getAttribute("href")).toBe("#apps");
    expect(within(attention).getByRole("link", { name: "Webhook pager" }).getAttribute("href")).toBe("#automations");
    // The widened parts show in their own sections, with the change in words above the list.
    const apps = screen.getByRole("region", { name: "Apps" });
    expect(within(apps).getByTestId("diff-change").textContent).toContain("App slack");
    expect(within(screen.getByRole("region", { name: "Automations" })).getByTestId("diff-change").textContent).toContain("Webhook pager");
    // The instructions diff sits inside the Instructions section.
    expect(within(screen.getByRole("region", { name: "Instructions" })).getByLabelText("Diff of Instructions")).toBeTruthy();
  });

  it("a first version says there is nothing to compare with", async () => {
    renderReview(listRoute({ current_version: null }));
    await screen.findByText("This is the first version: nothing to compare with.");
    expect(screen.queryByRole("region", { name: /What changed since/ })).toBeNull();
    // The content is still shown in full.
    expect(screen.getByRole("region", { name: "Instructions" }).textContent).toContain("Triage carefully.");
  });

  it("identical content says nothing changed", async () => {
    renderReview({
      "GET /v1/admin/agent-templates/schedule-manager/versions/1.1.0": () => json(200, { ...detail, content: offered.content }),
    });
    await screen.findByText("Nothing changed: this version matches 1.0.0.");
  });

  it("reviewing the version people use now shows no diff", async () => {
    renderReview(listRoute({ current_version: "1.1.0" }));
    await screen.findByRole("region", { name: "Instructions" });
    expect(screen.queryByRole("region", { name: /What changed since/ })).toBeNull();
    expect(screen.queryByText(/This is the first version/)).toBeNull();
  });

  it("Approve sends exactly the content_sha256 the page showed", async () => {
    const writes: { key: string; body: unknown }[] = [];
    renderReview({}, (key, body) => writes.push({ key, body }));
    await clickApprove();
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0].key).toBe("POST /v1/admin/agent-templates/schedule-manager/versions/1.1.0/approve");
    expect(writes[0].body).toEqual({ content_sha256: SHA_CONTENT });
  });

  it("a 409 on Approve reloads and says the version changed", async () => {
    let calls = 0;
    renderReview({
      "POST /v1/admin/agent-templates/schedule-manager/versions/1.1.0/approve": () => {
        calls += 1;
        return calls === 1 ? err(409, "template_content_changed", "This version's content changed since it was reviewed. Reload the review.") : json(200, detail);
      },
    });
    await clickApprove();
    await screen.findByText(/This version changed while you were reading/);
    // the review was reloaded from the server
    await waitFor(() => expect(calls).toBe(1));
  });

  it("Reject asks first, then sends nothing but the decision", async () => {
    const writes: { key: string; body: unknown }[] = [];
    renderReview({}, (key, body) => writes.push({ key, body }));
    fireEvent.click(await screen.findByRole("button", { name: "Reject" }));
    expect(writes).toHaveLength(0);
    const dlg = screen.getByRole("dialog", { name: "Reject Schedule manager 1.1.0?" });
    fireEvent.click(within(dlg).getByRole("button", { name: "Reject" }));
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0].key).toBe("POST /v1/admin/agent-templates/schedule-manager/versions/1.1.0/reject");
  });

  // FIX-271: every change the summary counts is visible in its section.
  const bin = (path: string, sha: string) => ({ path, size: 9, executable: false, sha256: sha.repeat(64), text: null });
  const txt = (path: string, text: string) => ({ path, size: text.length, executable: false, sha256: "a".repeat(64), text });
  const answer = (v: string, c: Record<string, unknown>) => ({
    [`GET /v1/admin/agent-templates/schedule-manager/versions/${v}`]: () => json(200, v === "1.0.0" ? { ...offered, content: c } : { ...detail, content: c }),
  });

  // FIX-275: the executable bit alone is a change the person must see before approving.
  it("a helper whose executable bit alone changed is shown, not folded, and says it is now runnable", async () => {
    const run = (executable: boolean) => ({ ...txt("helpers/run.sh", "#!/bin/sh\n"), executable });
    renderReview({
      ...answer("1.0.0", content({ helpers: [run(false)] })),
      ...answer("1.1.0", content({ helpers: [run(true)] })),
    });
    await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    const helpers = screen.getByRole("region", { name: "Helper files" });
    expect(within(helpers).queryByTestId("diff-unchanged")).toBeNull();
    expect(within(helpers).getByTestId("file-change").textContent).toMatch(/now runnable/i);
    expect(within(helpers).getByLabelText("Helper file helpers/run.sh").textContent).toContain("#!/bin/sh");
  });

  it("a skill file that changed and stopped being runnable shows the line next to its diff", async () => {
    const skill = (text: string, executable: boolean) => ({ name: "code-review", use_it_when: "A pull request is open", body: "Review carefully.\n", files: [{ ...txt("check.sh", text), executable }] });
    renderReview({
      ...answer("1.0.0", content({ skills: [skill("echo old\n", true)] })),
      ...answer("1.1.0", content({ skills: [skill("echo new\n", false)] })),
    });
    await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    const card = screen.getByRole("region", { name: "Skill code-review" });
    expect(within(card).getByLabelText("Diff of File check.sh of skill code-review").textContent).toContain("+ echo new");
    expect(within(card).getByTestId("file-change").textContent).toMatch(/no longer runnable/i);
  });

  it("shows removed skills, skill files and helper files in their sections, marked Removed", async () => {
    const kept = (files: unknown[]) => ({ name: "code-review", use_it_when: "A pull request is open", body: "Review carefully.\n", files });
    renderReview({
      ...answer("1.0.0", content({
        skills: [
          kept([txt("checklist.md", "- tests\n"), txt("old-notes.md", "old notes\n"), bin("logo.bin", "1")]),
          { name: "triage", use_it_when: "An issue arrives", body: "Sort the issue.\n", files: [txt("labels.md", "bug\n")] },
        ],
        helpers: [txt("scripts/run.sh", "#!/bin/sh\n"), txt("scripts/gone.sh", "echo gone\n"), bin("scripts/tool.bin", "2")],
      })),
      ...answer("1.1.0", content({ skills: [kept([txt("checklist.md", "- tests\n")])], helpers: [txt("scripts/run.sh", "#!/bin/sh\n")] })),
    });
    await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    const skills = screen.getByRole("region", { name: "Skills" });
    const gone = within(skills).getByRole("region", { name: "Removed skill triage" });
    expect(gone.textContent).toContain("Removed");
    expect(gone.textContent).toContain("An issue arrives");
    expect(document.body.textContent).not.toMatch(/Use it when/);
    expect(within(gone).getByLabelText("Diff of Skill triage body").textContent).toContain("- Sort the issue.");
    expect(within(gone).getByTestId("removed-file-labels.md").textContent).toContain("Removed");
    expect(within(gone).getByTestId("removed-file-labels.md").textContent).toContain("- bug");
    const card = within(skills).getByRole("region", { name: "Skill code-review" });
    expect(within(card).getByTestId("removed-file-old-notes.md").textContent).toContain("Removed");
    expect(within(card).getByTestId("removed-file-old-notes.md").textContent).toContain("- old notes");
    expect(within(card).getByTestId("removed-file-logo.bin").textContent).toContain("Removed");
    expect(within(card).getByTestId("removed-file-logo.bin").textContent).toContain("It is not text");
    const helpers = screen.getByRole("region", { name: "Helper files" });
    expect(within(helpers).getByTestId("removed-file-scripts/gone.sh").textContent).toContain("Removed");
    expect(within(helpers).getByTestId("removed-file-scripts/gone.sh").textContent).toContain("- echo gone");
    expect(within(helpers).getByTestId("removed-file-scripts/tool.bin").textContent).toContain("Removed");
  });

  it("an added skill reads as new, never as Unchanged", async () => {
    renderReview({
      ...answer("1.0.0", content({ skills: [], instructions: "Triage carefully." })),
      ...answer("1.1.0", content({ skills: [{ name: "release-notes", use_it_when: "A release is cut", body: "Write the notes.\n", files: [txt("style.md", "Be short.\n")] }] })),
    });
    await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    const card = screen.getByRole("region", { name: "Skill release-notes" });
    expect(within(card).queryByTestId("diff-unchanged")).toBeNull();
    for (const name of ["Diff of Skill release-notes body", "Diff of Skill release-notes · When to use it","Diff of File style.md of skill release-notes"]) {
      const diff = within(card).getByRole("region", { name });
      expect(diff.textContent).toContain("(added)");
    }
    expect(within(card).getAllByTestId("part-diff")).toHaveLength(2);
  });

  it("a memory that is only locked or unlocked says so; unlocking is looked at first", async () => {
    renderReview({
      ...answer("1.0.0", content({ memories: [{ key: "people", content: "Ada.", locked: false }, { key: "rules", content: "Be kind.", locked: true }] })),
      ...answer("1.1.0", content({ memories: [{ key: "people", content: "Ada.", locked: true }, { key: "rules", content: "Be kind.", locked: false }] })),
    });
    const block = await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    const [people, rules] = screen.getAllByTestId("template-memory");
    expect(within(people!).getByTestId("memory-lock-change").textContent).toBe("Now locked: the agent can read it but not change it.");
    expect(within(rules!).getByTestId("memory-lock-change").textContent).toBe("No longer locked.");
    // The text did not change: no empty diff.
    expect(within(screen.getByRole("region", { name: "Memories" })).queryByTestId("diff-view")).toBeNull();
    const attention = within(block).getByTestId("diff-attention");
    expect(attention.textContent).toContain("Memory rules — No longer locked.");
    expect(attention.textContent).not.toContain("Memory people");
  });

  it("a binary file that changed is said in its section, and a file that stops being text is not called removed", async () => {
    renderReview({
      ...answer("1.0.0", content({ helpers: [bin("scripts/tool.bin", "1"), txt("scripts/run.sh", "#!/bin/sh\n")] })),
      ...answer("1.1.0", content({ helpers: [bin("scripts/tool.bin", "2"), bin("scripts/run.sh", "3")] })),
    });
    await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    const helpers = screen.getByRole("region", { name: "Helper files" });
    expect(within(helpers).queryByTestId("diff-unchanged")).toBeNull();
    expect(within(helpers).getAllByTestId("file-change").map((e) => e.textContent)).toEqual([
      "It changed. It is not text, so there is no line comparison.",
      "It changed. It is not text, so there is no line comparison.",
    ]);
    expect(within(helpers).queryByText("Removed")).toBeNull();
  });

  it("Approve waits for the comparison: Loading while the offered version loads, an error with Retry when it fails", async () => {
    let release: (r: Response) => void = () => undefined;
    let calls = 0;
    renderReview({
      "GET /v1/admin/agent-templates/schedule-manager/versions/1.0.0": () => {
        calls += 1;
        return calls === 1 ? new Promise<Response>((resolve) => (release = resolve)) : json(200, offered);
      },
    });
    await screen.findByRole("region", { name: "Instructions" });
    await screen.findByText("Loading the comparison with 1.0.0…");
    expect((screen.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    release(err(500, "internal", "boom"));
    const retry = await screen.findByRole("button", { name: "Try again" });
    expect(screen.queryByText("Loading the comparison with 1.0.0…")).toBeNull();
    expect((screen.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    // Rejecting needs no comparison.
    expect((screen.getByRole("button", { name: "Reject" }) as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(retry);
    await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    await waitFor(() => expect((screen.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(false));
  });

  it("Approve waits for the list too: disabled while it loads, an error with Retry when it fails", async () => {
    let release: (r: Response) => void = () => undefined;
    let calls = 0;
    const ok = listRoute()["GET /v1/admin/agent-templates"];
    renderReview({
      "GET /v1/admin/agent-templates": () => {
        calls += 1;
        return calls === 1 ? new Promise<Response>((resolve) => (release = resolve)) : ok();
      },
    });
    await screen.findByRole("region", { name: "Instructions" });
    expect(screen.getByText("Loading the comparison…")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    release(err(500, "internal", "boom"));
    const retry = await screen.findByRole("button", { name: "Try again" });
    expect((screen.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(retry);
    await screen.findByRole("region", { name: /What changed since 1.0.0/ });
    await waitFor(() => expect((screen.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(false));
  });

  it("an offered version with no content states why there is no comparison", async () => {
    renderReview({ "GET /v1/admin/agent-templates/schedule-manager/versions/1.0.0": () => json(200, { ...offered, content: null }) });
    await screen.findByText("There is no comparison: 1.0.0 has no content to compare with.");
    expect((screen.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("a version that is not waiting for review shows its state in words and no decision", async () => {
    renderReview({
      "GET /v1/admin/agent-templates/schedule-manager/versions/1.1.0": () => json(200, { ...detail, status: "approved" }),
    });
    await screen.findByText("Approved");
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Reject" })).toBeNull();
  });
});
