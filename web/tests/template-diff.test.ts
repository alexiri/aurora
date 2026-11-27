import { describe, expect, it } from "vitest";

import type { TemplateContentView } from "@/lib/api/agent-templates";
import { diffTemplateContent } from "@/lib/template-diff";

// FIX-247: the pure diff of two template versions covers every part of `TemplateContentView`, each
// change with added/removed/changed, its label, old and new text for text parts, and the flag for the
// changes that widen what the agent can do or reach.

const content = (over: Record<string, unknown> = {}): TemplateContentView =>
  ({
    name: "Schedule manager",
    summary: "Triage and schedule the team's work",
    readme: "# Schedule manager\n",
    effort: null,
    min_core: null,
    agent: { name: "Daneel", may_message_first: false },
    instructions: "Triage the queue.",
    inputs: [],
    apps: [],
    skills: [],
    skill_refs: [],
    helpers: [],
    memories: [],
    automations: [],
    has_avatar: false,
    ...over,
  }) as unknown as TemplateContentView;

const skill = (name: string, over: Record<string, unknown> = {}) => ({
  name,
  use_it_when: `When a ${name} is open`,
  body: `${name} body\n`,
  files: [] as { path: string; size: number; executable: boolean; sha256: string; text: string | null }[],
  ...over,
});

const file = (path: string, text: string) => ({ path, size: text.length, executable: false, sha256: "a".repeat(64), text });

describe("diffTemplateContent", () => {
  it("identical content: no changes, nothing to look at first, every part unchanged", () => {
    const d = diffTemplateContent(content(), content());
    expect(d.changes).toEqual([]);
    expect(d.attention).toEqual([]);
    expect(d.summary).toBe("");
    expect(d.unchanged).toEqual([
      "Name", "Summary", "About this template", "Agent name", "May message first", "Effort", "Instructions",
      "Skills", "Skills from libraries", "Helper files", "Memories", "Automations", "Apps", "Inputs",
      "Minimum Aurora version", "Avatar",
    ]);
  });

  it("the text parts: name, summary, README, agent name, instructions", () => {
    const next = content({
      name: "Schedule keeper",
      summary: "Triage the work",
      readme: "# Schedule keeper\n",
      agent: { name: "Elijah", may_message_first: false },
      instructions: "Triage carefully.",
    });
    const d = diffTemplateContent(content(), next);
    const byPart = Object.fromEntries(d.changes.map((c) => [c.part, c]));
    expect(byPart["name"]).toMatchObject({ label: "Name", kind: "changed", old: "Schedule manager", new: "Schedule keeper" });
    expect(byPart["summary"]).toMatchObject({ label: "Summary", kind: "changed" });
    expect(byPart["readme"]).toMatchObject({ label: "About this template", kind: "changed" });
    expect(byPart["agent_name"]).toMatchObject({ label: "Agent name", kind: "changed", old: "Daneel", new: "Elijah" });
    expect(byPart["instructions"]).toMatchObject({ label: "Instructions", kind: "changed", old: "Triage the queue.", new: "Triage carefully." });
    expect(d.summary).toBe("5 changes: Instructions, the name, the summary, About this template, the agent's name");
    expect(d.unchanged).toContain("May message first");
  });

  it("may_message_first and effort change in words", () => {
    const d = diffTemplateContent(content(), content({ effort: "high", agent: { name: "Daneel", may_message_first: true } }));
    const byPart = Object.fromEntries(d.changes.map((c) => [c.part, c]));
    expect(byPart["may_message_first"]).toMatchObject({ label: "May message first", kind: "changed", detail: "It may message first." });
    expect(byPart["effort"]).toMatchObject({ label: "Effort", kind: "changed", detail: "The effort changes from the default to high." });
  });

  it("a skill added, removed, renamed, with its body, use-it-when and each file", () => {
    const base = content({ skills: [skill("code-review", { files: [file("checklist.md", "- tests\n")] }), skill("triage")] });
    const next = content({
      skills: [
        skill("code-review", { body: "Review carefully.\n", use_it_when: "When a pull request is open", files: [file("checklist.md", "- tests run\n"), file("notes.md", "notes\n")] }),
        skill("release-notes"),
      ],
    });
    const d = diffTemplateContent(base, next);
    const byPart = Object.fromEntries(d.changes.map((c) => [c.part, c]));
    // renamed: the old name removed, the new one added
    expect(byPart["skill:triage"]).toMatchObject({ label: "Skill triage", kind: "removed", old: "triage body\n" });
    expect(byPart["skill:release-notes"]).toMatchObject({ label: "Skill release-notes", kind: "added", new: "release-notes body\n", detail: "New in this version." });
    expect(byPart["skill:code-review:body"]).toMatchObject({ kind: "changed", old: "code-review body\n", new: "Review carefully.\n" });
    expect(byPart["skill:code-review:use_it_when"]).toMatchObject({ label: "Skill code-review · Use it when", kind: "changed" });
    expect(byPart["skill:code-review:file:checklist.md"]).toMatchObject({ label: "Skill code-review · checklist.md", kind: "changed", old: "- tests\n", new: "- tests run\n" });
    expect(byPart["skill:code-review:file:notes.md"]).toMatchObject({ kind: "added", new: "notes\n" });
    expect(d.summary).toBe("6 changes: 3 skills");
  });

  it("library skill references: added, removed, changed, with required/optional and where it comes from", () => {
    const base = content({
      skill_refs: [
        { kind: "library", address: "https://git.example.org/acme/skills.git", ref: "main", path: "skills", skill: "release-notes", required: true, why: "Writes release notes" },
        { kind: "organization", name: "org-notes", required: false, why: "Org notes" },
      ],
    });
    const next = content({
      skill_refs: [
        { kind: "library", address: "https://git.example.org/acme/skills.git", ref: "main", path: "skills", skill: "release-notes", required: false, why: "Writes release notes" },
        { kind: "organization", name: "org-notes", required: false, why: "Org notes" },
        { kind: "library", address: "https://git.example.org/acme/more.git", ref: null, path: null, skill: "postmortems", required: true, why: "Writes postmortems" },
      ],
    });
    const d = diffTemplateContent(base, next);
    const byPart = Object.fromEntries(d.changes.map((c) => [c.part, c]));
    expect(byPart["skill_ref:library:postmortems"]).toMatchObject({ label: "Library skill postmortems", kind: "added", attention: true, detail: expect.stringContaining("Writes postmortems") });
    expect(byPart["skill_ref:library:release-notes"]).toMatchObject({ label: "Library skill release-notes", kind: "changed", detail: "It is now optional." });
    expect(d.attention).toHaveLength(1);
    expect(d.summary).toBe("2 changes: 2 library skills");
  });

  it("helper files: added (look at these first), changed and removed", () => {
    const base = content({ helpers: [file("scripts/run.sh", "#!/bin/sh\n"), file("scripts/old.sh", "old\n")] });
    const next = content({ helpers: [file("scripts/run.sh", "#!/bin/sh\nset -e\n"), file("scripts/new.sh", "new\n")] });
    const d = diffTemplateContent(base, next);
    const byPart = Object.fromEntries(d.changes.map((c) => [c.part, c]));
    expect(byPart["helper:scripts/run.sh"]).toMatchObject({ label: "Helper file scripts/run.sh", kind: "changed", old: "#!/bin/sh\n", new: "#!/bin/sh\nset -e\n", attention: false });
    expect(byPart["helper:scripts/old.sh"]).toMatchObject({ kind: "removed", old: "old\n" });
    expect(byPart["helper:scripts/new.sh"]).toMatchObject({ kind: "added", attention: true, new: "new\n" });
    expect(d.attention.map((c) => c.part)).toEqual(["helper:scripts/new.sh"]);
    expect(d.summary).toBe("3 changes: 3 helper files");
  });

  it("memories: added, removed, changed content, and locked in words", () => {
    const base = content({
      memories: [
        { key: "people", content: "Ada leads the platform team.", locked: true },
        { key: "gone", content: "gone\n", locked: false },
        { key: "locked", content: "same\n", locked: false },
      ],
    });
    const next = content({
      memories: [
        { key: "people", content: "Ada leads the platform team. Bob runs the on-call.", locked: true },
        { key: "locked", content: "same\n", locked: true },
        { key: "fresh", content: "fresh\n", locked: false },
      ],
    });
    const d = diffTemplateContent(base, next);
    const byPart = Object.fromEntries(d.changes.map((c) => [c.part, c]));
    expect(byPart["memory:people"]).toMatchObject({ kind: "changed", old: "Ada leads the platform team.", new: "Ada leads the platform team. Bob runs the on-call." });
    expect(byPart["memory:gone"]).toMatchObject({ kind: "removed" });
    expect(byPart["memory:locked"]).toMatchObject({ kind: "changed", detail: "Now locked: the agent can read it but not change it." });
    expect(byPart["memory:locked"].old).toBeUndefined();
    expect(byPart["memory:fresh"]).toMatchObject({ kind: "added", new: "fresh\n" });
  });

  it("automations: a new schedule and webhook are looked at first; prompt, cron, instructions and verification in words", () => {
    const base = content({
      automations: [
        { kind: "schedule", name: "morning-triage", prompt: "Triage the queue.", cron: "0 9 * * *" },
        { kind: "webhook", name: "on-call", instructions: "Log it.", verification: "signed" },
        { kind: "schedule", name: "old", prompt: "old", cron: "0 8 * * *" },
      ],
    });
    const next = content({
      automations: [
        { kind: "schedule", name: "morning-triage", prompt: "Triage everything.", cron: "0 10 * * *" },
        { kind: "webhook", name: "on-call", instructions: "Log it.", verification: "password" },
        { kind: "schedule", name: "evening-sweep", prompt: "Sweep.", cron: "0 18 * * *" },
        { kind: "webhook", name: "pager", instructions: "Page.", verification: "none" },
      ],
    });
    const d = diffTemplateContent(base, next);
    const byPart = Object.fromEntries(d.changes.map((c) => [c.part, c]));
    expect(byPart["automation:schedule:evening-sweep"]).toMatchObject({ label: "Schedule evening-sweep", kind: "added", attention: true });
    expect(byPart["automation:webhook:pager"]).toMatchObject({ label: "Webhook pager", kind: "added", attention: true });
    expect(byPart["automation:schedule:morning-triage"]).toMatchObject({ kind: "changed", detail: "What it does changed. When it runs changed." });
    expect(byPart["automation:webhook:on-call"]).toMatchObject({ kind: "changed", detail: "How it is verified changed: now verified with a password." });
    expect(byPart["automation:schedule:old"]).toMatchObject({ kind: "removed" });
    expect(d.attention.map((c) => c.part)).toEqual(["automation:schedule:evening-sweep", "automation:webhook:pager"]);
    expect(d.summary).toBe("5 changes: 3 schedules, 2 webhooks");
  });

  it("apps: added, removed, widened and narrowed, the level in words, reordered is not a change", () => {
    const github = { app: "github", required: true, level: "ask", tools: null, why: "Reads the pull requests" };
    const base = content({ apps: [github, { app: "linear", required: false, level: "read", tools: null, why: "Reads issues" }] });
    const reordered = content({ apps: [{ app: "linear", required: false, level: "read", tools: null, why: "Reads issues" }, github] });
    expect(diffTemplateContent(base, reordered).changes).toEqual([]);

    const next = content({
      apps: [
        { app: "github", required: true, level: "act", tools: null, why: "Reads the pull requests" },
        { app: "slack", required: false, level: "read", tools: null, why: "Posts to the channel" },
      ],
    });
    const d = diffTemplateContent(base, next);
    const byPart = Object.fromEntries(d.changes.map((c) => [c.part, c]));
    expect(byPart["app:slack"]).toMatchObject({ label: "App slack", kind: "added", attention: true, detail: expect.stringContaining("Read only access") });
    expect(byPart["app:github"]).toMatchObject({ kind: "changed", attention: true, detail: "Access changes from Ask before changes to Change without asking." });
    expect(byPart["app:linear"]).toMatchObject({ kind: "removed", attention: false });
    expect(d.summary).toBe("3 changes: 3 apps");

    const narrowed = diffTemplateContent(base, content({ apps: [github, { app: "linear", required: false, level: "read", tools: null, why: "Reads issues" }, { app: "github2", required: true, level: "read", tools: null, why: "w" }] }));
    expect(narrowed.attention.map((c) => c.part)).toEqual(["app:github2"]);
  });

  it("custom access: more tools it may use is widened, and said in words", () => {
    const base = content({ apps: [{ app: "github", required: true, level: "ask", tools: { allow: ["list_issues"], ask: [] }, why: "w" }] });
    const next = content({ apps: [{ app: "github", required: true, level: "ask", tools: { allow: ["list_issues", "create_issue"], ask: [] }, why: "w" }] });
    const d = diffTemplateContent(base, next);
    expect(d.changes[0]).toMatchObject({ kind: "changed", attention: true, detail: expect.stringContaining("may use list_issues, create_issue") });
  });

  it("inputs: added, removed and changed are all looked at first, in words", () => {
    const base = content({
      inputs: [{ id: "repo", label: "Repository", help: null, kind: "text", required: true, pattern: null, max_length: null, options: null, default: null }],
    });
    const next = content({
      inputs: [
        { id: "repo", label: "Repository", help: null, kind: "text", required: false, pattern: null, max_length: null, options: null, default: "acme/agent-templates" },
        { id: "team", label: "Team", help: null, kind: "choice", required: true, pattern: null, max_length: null, options: ["platform", "ops"], default: null },
      ],
    });
    const d = diffTemplateContent(base, next);
    const byPart = Object.fromEntries(d.changes.map((c) => [c.part, c]));
    expect(byPart["input:repo"]).toMatchObject({ kind: "changed", attention: true, detail: "It is now optional. Its default changed." });
    expect(byPart["input:team"]).toMatchObject({ label: "Input Team", kind: "added", attention: true });
    expect(d.attention).toHaveLength(2);
    expect(d.summary).toBe("2 changes: 2 inputs");
  });

  it("a raised minimum Aurora version is looked at first, a lowered one is not", () => {
    const raised = diffTemplateContent(content({ min_core: "6.0.0" }), content({ min_core: "6.2.0" }));
    expect(raised.changes[0]).toMatchObject({ label: "Minimum Aurora version", attention: true, detail: "The oldest Aurora this version works with changes from 6.0.0 to 6.2.0." });
    const lowered = diffTemplateContent(content({ min_core: "6.2.0" }), content({ min_core: "6.0.0" }));
    expect(lowered.changes[0].attention).toBe(false);
  });

  it("the minimum version is compared in order: the first differing component decides", () => {
    const attention = (from: string, to: string) => diffTemplateContent(content({ min_core: from }), content({ min_core: to })).changes[0]!.attention;
    expect(attention("6.2.0", "5.9.9")).toBe(false);
    expect(attention("6.0.0", "6.2.0")).toBe(true);
    expect(attention("5.9.9", "6.0.0")).toBe(true);
    expect(attention("6.0", "6.0.1")).toBe(true);
    expect(attention("6.0.1", "6.0")).toBe(false);
  });

  it("unlocking a memory widens what the agent may change; locking it does not", () => {
    const m = (locked: boolean) => content({ memories: [{ key: "rules", content: "Be kind.", locked }] });
    expect(diffTemplateContent(m(true), m(false)).attention.map((c) => c.part)).toEqual(["memory:rules"]);
    expect(diffTemplateContent(m(false), m(true)).attention).toEqual([]);
  });

  it("binary files are compared by content: added, removed, changed and no longer text", () => {
    const bin = (path: string, sha: string) => ({ path, size: 4, executable: false, sha256: sha.repeat(64), text: null });
    const base = content({ helpers: [bin("gone.bin", "1"), bin("same.bin", "2"), bin("other.bin", "3"), file("was-text.sh", "x\n")] });
    const next = content({ helpers: [bin("same.bin", "2"), bin("other.bin", "4"), bin("was-text.sh", "5"), bin("new.bin", "6")] });
    const d = diffTemplateContent(base, next);
    expect(d.changes.map((c) => [c.part, c.kind])).toEqual([
      ["helper:gone.bin", "removed"],
      ["helper:other.bin", "changed"],
      ["helper:was-text.sh", "changed"],
      ["helper:new.bin", "added"],
    ]);
    expect(d.attention.map((c) => c.part)).toEqual(["helper:new.bin"]);
    const skills = diffTemplateContent(content({ skills: [skill("a", { files: [bin("x.bin", "1")] })] }), content({ skills: [skill("a")] }));
    expect(skills.changes.map((c) => [c.part, c.kind])).toEqual([["skill:a:file:x.bin", "removed"]]);
  });

  it("the executable bit is a change by itself, and is said next to a content change", () => {
    const run = (executable: boolean, text = "#!/bin/sh\n", sha = "a") => ({ ...file("run.sh", text), executable, sha256: sha.repeat(64) });
    const only = diffTemplateContent(content({ helpers: [run(false)] }), content({ helpers: [run(true)] }));
    expect(only.changes).toEqual([{ part: "helper:run.sh", group: "helpers", label: "Helper file run.sh", kind: "changed", attention: false, detail: "This file is now runnable." }]);
    expect(only.unchanged).not.toContain("Helper files (1)");

    const inSkill = diffTemplateContent(content({ skills: [skill("a", { files: [run(true)] })] }), content({ skills: [skill("a", { files: [run(false)] })] }));
    expect(inSkill.changes.map((c) => [c.part, c.detail])).toEqual([["skill:a:file:run.sh", "This file is no longer runnable."]]);
    expect(inSkill.summary).toBe("1 change: 1 skill");

    const both = diffTemplateContent(content({ helpers: [run(false)] }), content({ helpers: [run(true, "#!/bin/sh\nrm x\n", "b")] }));
    expect(both.changes[0]).toMatchObject({ old: "#!/bin/sh\n", new: "#!/bin/sh\nrm x\n", detail: "This file is now runnable." });

    // A file that was not text before and is a script now: both are said.
    const wasBinary = diffTemplateContent(content({ helpers: [{ ...run(false), text: null as unknown as string, sha256: "c".repeat(64) }] }), content({ helpers: [run(true)] }));
    expect(wasBinary.changes[0]?.detail).toBe("It changed. It was not text before, so there is no line comparison. This file is now runnable.");

    // A new or removed file has no earlier or later bit to compare with.
    const added = diffTemplateContent(content(), content({ helpers: [run(true)] }));
    expect(added.changes[0]?.detail).toBeUndefined();
  });

  it("the avatar arriving and leaving is said in words", () => {
    expect(diffTemplateContent(content(), content({ has_avatar: true })).changes[0]).toMatchObject({ label: "Avatar", kind: "added", detail: "The template now carries an avatar." });
    expect(diffTemplateContent(content({ has_avatar: true }), content()).changes[0]).toMatchObject({ label: "Avatar", kind: "removed", detail: "The template no longer carries an avatar." });
  });

  it("whitespace-only changes are shown, not hidden", () => {
    const d = diffTemplateContent(content(), content({ instructions: "Triage the queue. \n" }));
    expect(d.changes).toHaveLength(1);
    expect(d.changes[0]).toMatchObject({ label: "Instructions", kind: "changed", old: "Triage the queue.", new: "Triage the queue. \n" });
  });

  it("very large text is carried in full", () => {
    const big = "x".repeat(300_000);
    const d = diffTemplateContent(content(), content({ instructions: big }));
    expect(d.changes[0].new?.length).toBe(300_000);
  });

  it("the summary line in words, as the review page reads it", () => {
    const base = content({
      skills: [skill("a", { body: "old\n" }), skill("b", { body: "old\n" })],
    });
    const next = content({
      instructions: "Triage carefully.",
      skills: [skill("a", { body: "new\n" }), skill("b", { body: "new\n" })],
      apps: [{ app: "slack", required: false, level: "read", tools: null, why: "Posts" }],
      automations: [{ kind: "schedule", name: "morning-triage", prompt: "Triage.", cron: "0 9 * * *" }],
    });
    const d = diffTemplateContent(base, next);
    expect(d.summary).toBe("5 changes: Instructions, 2 skills, a new app, a new schedule");
    expect(d.unchanged).toEqual([
      "Name", "Summary", "About this template", "Agent name", "May message first", "Effort",
      "Skills from libraries", "Helper files", "Memories", "Inputs", "Minimum Aurora version", "Avatar",
    ]);
  });
});
