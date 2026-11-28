import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { parseSkillMd, readSkillFolder, type PickedFile } from "@/lib/skill-folder";
import { baseSkillName, descriptionSentence, newSkillName, skillDisplayName, useItToLine } from "@/lib/skill-name";
import { err, json } from "./support/api";
import { wrap } from "./support/render";
import { AgentSkills, applied, effective, me, openMenu, routes, skill } from "./support/skills";

// P5-REFINE-12 (behaviour-2) and P5-SKILLS-3 (skill-4, D-498): the names, the editor with its files folded, the Add
// file dialog and Upload a skill folder.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(),
}));

beforeEach(() => window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=skills"));

describe("skill folder names are made by Aurora (behaviour-2)", () => {
  it("makes kebab-case ASCII from the name, at most 64 characters", () => {
    expect(baseSkillName("Weekly report")).toBe("weekly-report");
    expect(baseSkillName("Crème brûlée!")).toBe("creme-brulee");
    expect(baseSkillName("日本語")).toBe("skill");
    expect(baseSkillName("x".repeat(100)).length).toBe(64);
  });
  it("never starts with the reserved aurora-", () => {
    expect(baseSkillName("Aurora tips")).toBe("my-aurora-tips");
    expect(baseSkillName("aurora")).toBe("my-aurora");
    expect(baseSkillName("Auroral light")).toBe("auroral-light");
  });
  it("is unique by a numeric suffix, and a retry moves on", () => {
    expect(newSkillName("Weekly report", ["weekly-report"])).toBe("weekly-report-2");
    expect(newSkillName("Weekly report", ["weekly-report", "weekly-report-2"])).toBe("weekly-report-3");
    expect(newSkillName("Weekly report", [], 1)).toBe("weekly-report-2");
    expect(newSkillName("x".repeat(100), ["x".repeat(64)]).length).toBeLessThanOrEqual(64);
  });
});

describe("one spelling of a skill's name and sentence (D-498)", () => {
  it("is the title, else the folder name made readable, a one-word name as written", () => {
    expect(skillDisplayName("weekly-report")).toBe("Weekly report");
    expect(skillDisplayName("internal-comms", null)).toBe("Internal comms");
    expect(skillDisplayName("pdf")).toBe("pdf");
    expect(skillDisplayName("summarize-pr", "Summarize pull requests")).toBe("Summarize pull requests");
    expect(skillDisplayName("summarize-pr", "  ")).toBe("Summarize pr");
  });
  it("capitalizes a description in a list and completes 'Use it to …' in the panel", () => {
    expect(descriptionSentence("draft a short email.")).toBe("Draft a short email.");
    expect(useItToLine("Draft a short email.")).toBe("Use it to draft a short email.");
    expect(useItToLine("PR summaries for a reviewer")).toBe("Use it to PR summaries for a reviewer");
    // a sentence of its own is never glued to the label
    expect(useItToLine("Use it when asked.")).toBe("Use it when asked.");
    expect(useItToLine("when asked for a report")).toBe("When asked for a report");
    expect(useItToLine("someone asks for a report")).toBe("Someone asks for a report");
    // repository and app text stays as written
    expect(useItToLine("Comprehensive PDF toolkit.", true)).toBe("Comprehensive PDF toolkit.");
  });
});

const summarize = skill({
  id: "s1",
  name: "summarize-pull-requests",
  title: "Summarize pull requests",
  description: "Summarize a pull request.",
  body: "Five bullets.",
  files: [{ path: "template.md", size: 9, executable: false }],
});

const base = (items: unknown[], extra: Parameters<typeof routes>[0] = {}) => ({
  "GET /v1/me": () => json(200, me()),
  "GET /v1/agents/a1/skills/effective": effective(items),
  "GET /v1/orgs/o1/skill-libraries": () => json(200, { items: [] }),
  "GET /v1/agents/a1/skill-library-updates": () => json(200, { items: [] }),
  "GET /v1/skills/s1/files/template.md": () => new Response("## Summary"),
  ...extra,
});

async function startNew() {
  await screen.findByRole("button", { name: "Add skill" });
  openMenu("Add skill");
  fireEvent.click(screen.getByRole("menuitem", { name: "Write a new skill" }));
  return screen.findByRole("form", { name: "New skill" });
}

async function edit() {
  fireEvent.click(await screen.findByRole("button", { name: "Summarize pull requests" }));
  fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
  return screen.findByRole("form", { name: "Edit Summarize pull requests" });
}

const fill = (form: HTMLElement, name: string, when: string, instructions = "") => {
  fireEvent.change(within(form).getByLabelText("Name"), { target: { value: name } });
  fireEvent.change(within(form).getByLabelText("When should Daneel use it?"), { target: { value: when } });
  if (instructions) fireEvent.change(within(form).getByRole("textbox", { name: "Instructions" }), { target: { value: instructions } });
};

const writes = (calls: ReturnType<typeof routes>) => calls.filter((c) => c.method !== "GET").map((c) => `${c.method} ${c.path}${c.query}`);

describe("the editor (skill-4)", () => {
  it("is drawn as the mockup says: the folder made from the name, the sentence hint, Write / Preview and the count", async () => {
    routes(base([]));
    wrap(<AgentSkills />);
    const form = await startNew();
    expect(within(form).queryByText(/Folder:/)).toBeNull();
    fill(form, "Weekly report", "write the Friday status report", "List wins.");
    expect(within(form).getByText("weekly-report")).toBeTruthy();
    expect(form.textContent).toContain("Finish “Use it to …”. Example: write the Friday status report from Jira and GitHub.");
    expect(within(form).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Write", "Preview"]);
    expect(within(form).getByText("Markdown · 10 of 48,000 characters")).toBeTruthy();
    expect(within(form).queryByRole("switch")).toBeNull();
    expect(form.querySelector('input[type="file"]:not(.sr-only)')).toBeNull();
    expect(screen.getByRole("region", { name: "Unsaved changes" }).textContent).toContain("New skill · not saved yet");
  });

  it("creates by title, the folder name made by Aurora, on by default", async () => {
    const calls = routes(base([], { "POST /v1/agents/a1/skills": () => json(201, skill({ id: "new", name: "weekly-report" })) }));
    wrap(<AgentSkills />);
    fill(await startNew(), "Weekly report", "write the Friday status report", "List wins.");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(writes(calls)).toEqual(["POST /v1/agents/a1/skills"]));
    expect(JSON.parse(calls.find((c) => c.method === "POST")!.body)).toEqual({ name: "weekly-report", title: "Weekly report", description: "write the Friday status report", body: "List wins.", enabled: true });
  });

  it("asks for a name and when to use it before it writes anything", async () => {
    const calls = routes(base([]));
    wrap(<AgentSkills />);
    await startNew();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Give the skill a name.")).toBeTruthy();
    expect(screen.getByText("Say when Daneel should use it.")).toBeTruthy();
    expect(writes(calls)).toEqual([]);
  });

  it("retries once with the next suffix when the folder name is taken", async () => {
    const names: string[] = [];
    routes(
      base([], {
        "POST /v1/agents/a1/skills": async (r) => {
          const b = (await r.json()) as { name: string };
          names.push(b.name);
          return names.length === 1 ? err(409, "conflict", "A skill with this name already exists in this scope.") : json(201, skill({ name: b.name }));
        },
      }),
    );
    wrap(<AgentSkills />);
    fill(await startNew(), "Weekly report", "asked");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(names).toEqual(["weekly-report", "weekly-report-2"]));
  });

  it("edits in the panel: the folder is fixed, Save sends only what changed, Discard drops it", async () => {
    const calls = routes(base([applied(summarize)], { "PATCH /v1/skills/s1": () => json(200, summarize) }));
    wrap(<AgentSkills />);
    const form = await edit();
    expect(within(form).getByText("summarize-pull-requests")).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Unsaved changes" })).toBeNull();
    expect(within(form).getByRole("button", { name: "Done" })).toBeTruthy();
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "Summarize PRs" } });
    // the folder never changes with the name
    expect(within(form).getByText("summarize-pull-requests")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(writes(calls)).toEqual(["PATCH /v1/skills/s1"]));
    expect(calls.find((c) => c.method === "PATCH")!.body).toBe('{"title":"Summarize PRs"}');
  });

  it("asks before leaving with unsaved changes", async () => {
    routes(base([applied(summarize), applied(skill({ name: "pdf" }))]));
    wrap(<AgentSkills />);
    const form = await edit();
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "Other" } });
    fireEvent.click(within(screen.getByTestId("skill-row-pdf")).getByRole("button", { name: "pdf" }));
    const dlg = await screen.findByRole("dialog", { name: /Leave without saving/ });
    fireEvent.click(within(dlg).getByRole("button", { name: /Keep editing/ }));
    expect(screen.getByRole("form", { name: "Edit Summarize pull requests" })).toBeTruthy();
  });
});

describe("files, folded to one line", () => {
  it("names the files in one line; opened, a file is read and edited in place and saved with the form", async () => {
    const calls = routes(base([applied(summarize)], { "PUT /v1/skills/s1/files/template.md": () => json(200, summarize) }));
    wrap(<AgentSkills />);
    const form = await edit();
    const files = within(form).getByRole("region", { name: "Files" });
    const fold = within(files).getByRole("button", { name: /Files · 2/ });
    expect(fold).toHaveAttribute("aria-expanded", "false");
    expect(fold.textContent).toContain("SKILL.md, template.md");
    expect(within(files).queryByRole("button", { name: "Open template.md" })).toBeNull();
    fireEvent.click(fold);
    expect(within(files).getByRole("button", { name: "Edit above" })).toBeTruthy();
    fireEvent.click(within(files).getByRole("button", { name: "Open template.md" }));
    const area = (await within(files).findByRole("textbox", { name: "Content of template.md" })) as HTMLTextAreaElement;
    expect(area.value).toBe("## Summary");
    fireEvent.change(area, { target: { value: "## Done" } });
    expect(within(files).getByRole("button", { name: "Close template.md" })).toBeTruthy();
    expect(writes(calls)).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    // no `executable` in the query: the file keeps its bit
    await waitFor(() => expect(writes(calls)).toEqual(["PUT /v1/skills/s1/files/template.md"]));
    expect(calls.find((c) => c.method === "PUT")!.body).toBe("## Done");
  });

  it("deletes a file after a confirm", async () => {
    const calls = routes(base([applied(summarize)], { "DELETE /v1/skills/s1/files/template.md": () => json(204, null) }));
    wrap(<AgentSkills />);
    const files = within(await edit()).getByRole("region", { name: "Files" });
    fireEvent.click(within(files).getByRole("button", { name: /Files · 2/ }));
    fireEvent.click(within(files).getByRole("button", { name: "Delete template.md" }));
    const dlg = await screen.findByRole("dialog", { name: "Delete the file “template.md”?" });
    expect(writes(calls)).toEqual([]);
    fireEvent.click(within(dlg).getByRole("button", { name: "Delete file" }));
    await waitFor(() => expect(writes(calls)).toEqual(["DELETE /v1/skills/s1/files/template.md"]));
    await waitFor(() => expect(within(files).getByRole("button", { name: /Files · 1/ })).toBeTruthy());
  });

  it("Add a file is a dialog: the server's path rules, the executable bit, and the content as typed", async () => {
    const calls = routes(base([applied(summarize)], { "PUT /v1/skills/s1/files/scripts/pr-diff.sh": () => json(200, summarize) }));
    wrap(<AgentSkills />);
    const form = await edit();
    expect(within(form).queryByLabelText("File name")).toBeNull();
    fireEvent.click(within(form).getByRole("button", { name: "Add file" }));
    const dlg = await screen.findByRole("dialog", { name: "Add a file" });
    expect(within(dlg).getByText("Relative to the skill’s folder.")).toBeTruthy();
    expect(dlg.textContent).toContain("Paste the file here, or upload one");
    expect(within(dlg).getByText("Daneel can run it from the computer.")).toBeTruthy();
    const name = within(dlg).getByLabelText("File name");
    const add = within(dlg).getByRole("button", { name: "Add file" });
    for (const [path, words] of [
      ["../evil", "The path must not contain empty, '.' or '..' segments."],
      ["a/b/c/d/e.sh", "The path may have at most 4 levels."],
      ["my file.md", "Use only letters, digits, dots, dashes, underscores and slashes."],
      ["docs/skill.md", "SKILL.md is the skill's instructions; edit them above."],
      [`${"a".repeat(201)}`, "The path may have at most 200 characters."],
    ]) {
      fireEvent.change(name, { target: { value: path } });
      fireEvent.click(add);
      expect(await within(dlg).findByText(words)).toBeTruthy();
    }
    fireEvent.change(name, { target: { value: "scripts/pr-diff.sh" } });
    fireEvent.change(within(dlg).getByLabelText("Content"), { target: { value: "x".repeat(300_000) } });
    fireEvent.click(add);
    expect(await within(dlg).findByText("A skill's instructions and files can total at most 300,000 characters.")).toBeTruthy();
    expect(writes(calls)).toEqual([]);
    fireEvent.change(within(dlg).getByLabelText("Content"), { target: { value: "echo hi" } });
    fireEvent.click(within(dlg).getByRole("switch", { name: "Executable" }));
    fireEvent.click(add);
    await waitFor(() => expect(writes(calls)).toEqual(["PUT /v1/skills/s1/files/scripts/pr-diff.sh?executable=true"]));
    expect(calls.find((c) => c.method === "PUT")!.body).toBe("echo hi");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add a file" })).toBeNull());
  });

  it("a skill has at most 50 files besides SKILL.md", async () => {
    const full = skill({ ...summarize, files: Array.from({ length: 50 }, (_, i) => ({ path: `f${i}.md`, size: 1, executable: false })) });
    routes(base([applied(full)]));
    wrap(<AgentSkills />);
    fireEvent.click(within(await edit()).getByRole("button", { name: "Add file" }));
    const dlg = await screen.findByRole("dialog", { name: "Add a file" });
    fireEvent.change(within(dlg).getByLabelText("File name"), { target: { value: "one-more.md" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Add file" }));
    expect(await within(dlg).findByText("A skill can have at most 50 files.")).toBeTruthy();
  });

  it("a new skill's files are written after it, switched on only once all are there", async () => {
    const calls = routes(
      base([], {
        "POST /v1/agents/a1/skills": () => json(201, skill({ id: "new", enabled: false })),
        "PUT /v1/skills/new/files/a.md": () => json(200, skill({ id: "new" })),
        "PATCH /v1/skills/new": () => json(200, skill({ id: "new" })),
      }),
    );
    wrap(<AgentSkills />);
    const form = await startNew();
    fill(form, "Weekly report", "write it");
    fireEvent.click(within(form).getByRole("button", { name: "Add file" }));
    const dlg = await screen.findByRole("dialog", { name: "Add a file" });
    fireEvent.change(within(dlg).getByLabelText("File name"), { target: { value: "a.md" } });
    fireEvent.change(within(dlg).getByLabelText("Content"), { target: { value: "A" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Add file" }));
    await waitFor(() => expect(within(form).getByRole("button", { name: /Files · 2/ }).textContent).toContain("SKILL.md, a.md"));
    expect(writes(calls)).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(writes(calls)).toEqual(["POST /v1/agents/a1/skills", "PUT /v1/skills/new/files/a.md?executable=false", "PATCH /v1/skills/new"]));
    expect(JSON.parse(calls.find((c) => c.method === "POST")!.body).enabled).toBe(false);
  });
});

// ---- upload a skill folder ------------------------------------------------------------------------

const picked = (files: Record<string, string>, folder = "release-notes"): PickedFile[] =>
  Object.entries(files).map(([path, text]) => {
    const bytes = new TextEncoder().encode(text);
    return { path: `${folder}/${path}`, size: bytes.length, read: async () => bytes };
  });
const SKILL_MD = "---\nname: release-notes\ndescription: >\n  Write release notes\n  from merged pull requests.\nlicense: MIT\n---\n\nCollect them.\n";

describe("reading a skill folder", () => {
  it("reads the front matter, the body and the files; a script is executable", async () => {
    expect(parseSkillMd('---\nname: "a"\ndescription: Do it: now\n---\nBody')).toEqual({ fields: { name: "a", description: "Do it: now" }, body: "Body" });
    const result = await readSkillFolder(picked({ "SKILL.md": SKILL_MD, "scripts/run.sh": "#!/bin/sh\n", "notes.md": "n", ".DS_Store": "x" }), [], "Daneel");
    expect(result).toMatchObject({
      ok: true,
      folder: { name: "release-notes", description: "Write release notes from merged pull requests.", body: "Collect them.\n", skipped: [".DS_Store"] },
    });
    if (!result.ok) throw new Error("not ok");
    expect(result.folder.files.map((f) => [f.path, f.executable])).toEqual([["notes.md", false], ["scripts/run.sh", true]]);
  });

  it("takes a description of up to 1,024 characters, the open skill format's maximum", async () => {
    const md = (n: number) => `---\nname: release-notes\ndescription: ${"d".repeat(n)}\n---\n\nbody\n`;
    expect(await readSkillFolder(picked({ "SKILL.md": md(1_024) }), [], "Daneel")).toMatchObject({ ok: true });
    expect(await readSkillFolder(picked({ "SKILL.md": md(1_025) }), [], "Daneel")).toEqual({
      ok: false,
      problems: ["SKILL.md: the description is longer than 1024 characters."],
    });
  });

  it("names every problem per file, in plain words", async () => {
    const none = await readSkillFolder(picked({ "readme.md": "x" }), [], "Daneel");
    expect(none).toEqual({ ok: false, problems: ["The folder has no SKILL.md at its top level."] });
    const bad = await readSkillFolder(picked({ "SKILL.md": "---\nname: release-notes\n---\nbody", "my file.md": "x", "a/b/c/d/e.md": "y", "big.txt": "z".repeat(301_000) }), ["release-notes"], "Daneel");
    expect(bad.ok).toBe(false);
    const words = bad.ok ? "" : bad.problems.join("\n");
    expect(words).toContain("SKILL.md: its front matter needs a description.");
    expect(words).toContain("my file.md: Use only letters");
    expect(words).toContain("a/b/c/d/e.md: The path may have at most 4 levels.");
    expect(words).toContain("Daneel already has a skill in the folder “release-notes”");
    expect(words).toContain("a skill can total at most 300,000.");
  });
});

describe("Upload a skill folder", () => {
  const choose = (dlg: HTMLElement, files: Record<string, string>) => {
    const list = Object.entries(files).map(([path, text]) => {
      const f = new File([text], path.split("/").pop()!);
      Object.defineProperty(f, "webkitRelativePath", { value: `release-notes/${path}` });
      // jsdom's File has no arrayBuffer
      Object.defineProperty(f, "arrayBuffer", { value: async () => new TextEncoder().encode(text).buffer });
      return f;
    });
    fireEvent.change(within(dlg).getByTestId("skill-folder-input"), { target: { files: list } });
  };

  async function openUpload() {
    await screen.findByRole("button", { name: "Add skill" });
    openMenu("Add skill");
    fireEvent.click(screen.getByRole("menuitem", { name: "Upload a skill folder" }));
    return screen.findByRole("dialog", { name: "Upload a skill folder" });
  }

  it("shows what will be created, then creates the skill and its files", async () => {
    const calls = routes(
      base([], {
        "POST /v1/agents/a1/skills": () => json(201, skill({ id: "new", enabled: false })),
        "PUT /v1/skills/new/files/scripts/run.sh": () => json(200, skill({ id: "new" })),
        "PATCH /v1/skills/new": () => json(200, skill({ id: "new" })),
      }),
    );
    wrap(<AgentSkills />);
    const dlg = await openUpload();
    expect(within(dlg).getByTestId("skill-folder-input")).toHaveAttribute("webkitdirectory");
    choose(dlg, { "SKILL.md": SKILL_MD, "scripts/run.sh": "#!/bin/sh\necho hi\n" });
    const summary = await within(dlg).findByRole("region", { name: "What will be created" });
    expect(summary.textContent).toContain("Release notes");
    expect(summary.textContent).toContain("Folder: release-notes · 2 files");
    expect(within(summary).getByText("scripts/run.sh")).toBeTruthy();
    expect(writes(calls)).toEqual([]);
    fireEvent.click(within(dlg).getByRole("button", { name: "Create skill" }));
    await waitFor(() => expect(writes(calls)).toEqual(["POST /v1/agents/a1/skills", "PUT /v1/skills/new/files/scripts/run.sh?executable=true", "PATCH /v1/skills/new"]));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Upload a skill folder" })).toBeNull());
  });

  it("a folder with a problem creates nothing", async () => {
    const calls = routes(base([]));
    wrap(<AgentSkills />);
    const dlg = await openUpload();
    choose(dlg, { "notes.md": "x" });
    expect(await within(dlg).findByText("The folder has no SKILL.md at its top level.")).toBeTruthy();
    expect(within(dlg).getByRole("button", { name: "Create skill" })).toBeDisabled();
    expect(writes(calls)).toEqual([]);
  });

  it("removes the half-made skill when a file is refused, and names the file", async () => {
    const calls = routes(
      base([], {
        "POST /v1/agents/a1/skills": () => json(201, skill({ id: "new", enabled: false })),
        "PUT /v1/skills/new/files/notes.md": () => err(422, "skill_too_large", "Too large."),
        "DELETE /v1/skills/new": () => json(204, null),
      }),
    );
    wrap(<AgentSkills />);
    const dlg = await openUpload();
    choose(dlg, { "SKILL.md": SKILL_MD, "notes.md": "n" });
    await within(dlg).findByRole("region", { name: "What will be created" });
    fireEvent.click(within(dlg).getByRole("button", { name: "Create skill" }));
    expect(await within(dlg).findByText(/“notes\.md” couldn’t be saved, so no skill was created/)).toBeTruthy();
    expect(writes(calls)).toEqual(["POST /v1/agents/a1/skills", "PUT /v1/skills/new/files/notes.md?executable=false", "DELETE /v1/skills/new"]);
  });
});
