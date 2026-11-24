import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { TemplateVersionReview } from "@/components/templates/review-version";
import { err, json, type Handler } from "./support/api";
import { wrap } from "./support/render";

// C-1114: the review of one version — everything in full as data, marked as text from the source at
// the commit, the README with its Preview toggle, the diff against the offered version, Approve bound
// to the exact content_sha256 the page showed (a 409 reloads with the version-changed sentence), and
// Reject with a confirm.

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

const listRoute = () => ({
  "GET /v1/admin/agent-templates": () =>
    json(200, {
      items: [{ template_id: "schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work", source: { id: "s1", name: "Team templates" }, current_version: "1.0.0", offered: true, head_state: "ok", head_version: null, head_problems: null, removed_upstream: false, needs_apps: [], needs_libraries: [], versions: [] }],
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
    const helper = screen.getByLabelText("Helper file scripts/run.sh").parentElement!;
    expect(helper.textContent).toContain("executable");
    expect(helper.textContent).toContain("#!/bin/sh");
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

  it("says people keep using the offered version, and diffs against it", async () => {
    renderReview();
    await screen.findByText("People keep using 1.0.0 until you approve this one.");
    const diff = await screen.findByRole("region", { name: /Changes against 1.0.0/ });
    expect(within(diff).getByText("- Triage the queue.")).toBeTruthy();
    expect(within(diff).getByText("+ Triage carefully.")).toBeTruthy();
  });

  it("Approve sends exactly the content_sha256 the page showed", async () => {
    const writes: { key: string; body: unknown }[] = [];
    renderReview({}, (key, body) => writes.push({ key, body }));
    fireEvent.click(await screen.findByRole("button", { name: "Approve" }));
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
    fireEvent.click(await screen.findByRole("button", { name: "Approve" }));
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

  it("a version that is not waiting for review shows its state in words and no decision", async () => {
    renderReview({
      "GET /v1/admin/agent-templates/schedule-manager/versions/1.1.0": () => json(200, { ...detail, status: "approved" }),
    });
    await screen.findByText("Approved");
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Reject" })).toBeNull();
  });
});
