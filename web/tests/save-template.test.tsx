import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AgentExportImport } from "@/components/agents/export-import";
import { outlinePlaceholder, SaveTemplateDialog } from "@/components/agents/save-template";
import { seedMoreControls } from "./support/render";
import { json, mockFetch } from "./support/api";

// P6-TPL-5 (C-1118): Save as template on the Manage page. The dialog reads the agent through the
// existing routes, ticks groups (memories unticked by default), needs "What it's for" and a why per
// app, and downloads <id>-<version>.zip built by lib/export/agent-template.ts.

const agent = {
  id: "a1",
  name: "Daneel",
  handle: "daneel",
  brief: "Triage the team's pull requests.",
  model_id: null,
  proactive_messaging: "ask",
  org_id: "org1",
  workspace_id: "ws1",
} as never;

const ownSkill = { id: "sk-own", name: "release-notes", description: "When asked what shipped", body: "# Notes", files: [], tier: "agent", source: null, enabled: true };
const libSkill = {
  id: "sk-lib",
  name: "incident-writeup",
  description: "Writes the incident summary.",
  body: "Library body that is never written.",
  files: [],
  tier: "agent",
  enabled: true,
  source: { kind: "library", detached: false, library_id: "lib1", update_pending: false },
};
const library = { id: "lib1", label: "Service desk skills", scope: "org", status: "active", url: "https://gitlab.example.org/agents/skills.git", ref: "v1.2", path: "skills" };

function serve(extra: { accounts?: unknown[]; grants?: unknown[]; helpers?: unknown[]; helperBodies?: Record<string, BodyInit>; skills?: unknown[]; effective?: unknown[]; libraries?: Record<string, unknown> } = {}) {
  return mockFetch({
    "GET /v1/agents/a1/memories": () => json(200, { items: extraMemories, next_cursor: null }),
    "GET /v1/agents/a1/skills": () => json(200, { items: extra.skills ?? [ownSkill, libSkill], next_cursor: null }),
    "GET /v1/agents/a1/schedules": () => json(200, { items: extraSchedules, next_cursor: null }),
    "GET /v1/agents/a1/webhooks": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/agents/a1/grants": () => json(200, { connections: extra.grants ?? [], secrets: [], converted_by_app: [] }),
    "GET /v1/workspaces/ws1/accounts": () => json(200, { items: extra.accounts ?? [], next_cursor: null }),
    "GET /v1/agents/a1/helpers": () => json(200, { items: extra.helpers ?? [] }),
    "GET /v1/agents/a1/helpers/content": (req) => {
      const path = new URL(req.url).searchParams.get("path") ?? "";
      const body = extra.helperBodies?.[path];
      return body === undefined ? json(200, "") : new Response(body, { status: 200 });
    },
    "GET /v1/agents/a1/skills/effective": () => json(200, { items: extra.effective ?? [], shadowed: [] }),
    "GET /v1/skill-libraries/lib1": () => json(200, extra.libraries?.lib1 ?? library),
  });
}

let extraMemories: unknown[] = [];
let extraSchedules: unknown[] = [];

const githubAccount = { kind: "connection", app: { app_id: "github" }, ref: { type: "connection", id: "c1" }, agents: [{ agent: { id: "a1" }, level: "act" }] };
const linearAccount = { kind: "connection", app: { app_id: "linear" }, ref: { type: "connection", id: "c2" }, agents: [{ agent: { id: "a1" }, level: "custom" }] };
const linearGrant = { agent_id: "a1", connection_id: "c2", mode: "custom", allow_tools: ["linear_issue_get"], ask_tools: ["linear_issue_create"] };

function mount(ui = <SaveTemplateDialog agent={agent} onClose={() => undefined} />) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seedMoreControls(qc, true);
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

const dialog = () => screen.getByRole("dialog", { name: "Save Daneel as a template" });

async function ready(extra = {}) {
  serve(extra);
  mount();
  const d = dialog();
  await waitFor(() => expect(within(d).getByTestId("template-groups")).toBeInTheDocument());
  return d;
}

/** Fills Summary, the why of every app row (if any) and the README replacement, so only Save's result is under test. */
function fillRequired(d: HTMLElement, { why = true, readme = true }: { why?: boolean; readme?: boolean } = {}) {
  fireEvent.change(within(d).getByLabelText("Summary"), { target: { value: "Triage the team's pull requests." } });
  if (why) for (const input of within(d).queryAllByLabelText(/Why it's needed/)) fireEvent.change(input, { target: { value: "Keeps the queue moving." } });
  if (readme) {
    const area = within(d).getByLabelText("What it's for") as HTMLTextAreaElement;
    fireEvent.change(area, { target: { value: area.value.replace(outlinePlaceholder("Daneel"), "Triage.") } });
  }
}

const blobs: Blob[] = [];
const clicked: string[] = [];

beforeEach(() => {
  extraMemories = [];
  extraSchedules = [];
  blobs.length = 0;
  clicked.length = 0;
  URL.createObjectURL = vi.fn((b: Blob | MediaSource) => (blobs.push(b as Blob), "blob:x"));
  URL.revokeObjectURL = vi.fn();
  vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
    clicked.push(this.download);
  });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

async function zipText(which = 0): Promise<string> {
  const bytes = new Uint8Array(
    await new Promise<ArrayBuffer>((resolve) => {
      const r = new FileReader();
      r.onload = () => resolve(r.result as ArrayBuffer);
      r.readAsArrayBuffer(blobs[which]!);
    }),
  );
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
}

describe("Save as template", () => {
  it("sits beside Export on the Manage page and opens the dialog", async () => {
    serve();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    seedMoreControls(qc, true);
    render(
      <QueryClientProvider client={qc}>
        <AgentExportImport agent={agent} />
      </QueryClientProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Save as template…" }));
    expect(await screen.findByRole("dialog", { name: "Save Daneel as a template" })).toBeInTheDocument();
  });

  it("prefills name, id, version and the outline; the id follows the name until edited", async () => {
    const d = await ready();
    expect(within(d).getByLabelText("Template name")).toHaveValue("Daneel");
    expect(within(d).getByLabelText("Id")).toHaveValue("daneel");
    expect(within(d).getByLabelText("Version")).toHaveValue("1.0.0");
    const area = within(d).getByLabelText("What it's for") as HTMLTextAreaElement;
    expect(area.value).toContain("## What Daneel does");
    expect(area.value).toContain(outlinePlaceholder("Daneel"));
    fireEvent.change(within(d).getByLabelText("Template name"), { target: { value: "Queue Guard" } });
    expect(within(d).getByLabelText("Id")).toHaveValue("queue-guard");
    fireEvent.change(within(d).getByLabelText("Id"), { target: { value: "pr-triage" } });
    fireEvent.change(within(d).getByLabelText("Template name"), { target: { value: "Rex" } });
    expect(within(d).getByLabelText("Id")).toHaveValue("pr-triage");
  });

  it("refuses Save while the outline's placeholder lines are unchanged, then downloads <id>-<version>.zip", async () => {
    const d = await ready({ accounts: [githubAccount] });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(await within(d).findByTestId("save-problem")).toHaveTextContent(/Replace the placeholder lines/);
    expect(blobs).toHaveLength(0);
    fillRequired(d);
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(clicked).toEqual(["daneel-1.0.0.zip"]);
    const raw = await zipText();
    expect(raw).toContain("format: 1");
    expect(raw).toContain("id: daneel");
    expect(raw).toContain("Triage.");
    expect(raw).toContain("Made with Save as template on");
  });

  it("ticks memories off by default with its sentence, and writes a ticked locked memory locked (C-1151)", async () => {
    extraMemories = [
      { key: "likes-tea", content: "tea", updated_at: "2026-11-01T09:00:00Z" },
      { key: "pinned", content: "Rules.", updated_at: "2026-11-01T09:00:00Z", locked: true },
    ];
    const d = await ready();
    const memories = within(d).getByLabelText("2 memories");
    expect(memories).not.toBeChecked();
    expect(within(d).getByText(/Memories can hold what Daneel learned about people/)).toBeInTheDocument();
    fillRequired(d);
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(clicked).toHaveLength(1);
    expect(await zipText()).not.toContain("likes-tea");
    fireEvent.click(memories);
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(await zipText(1)).toContain("locked: true");
  });

  it("writes the apps with their level or custom tools, and needs a why per app", async () => {
    const d = await ready({ accounts: [githubAccount, linearAccount], grants: [linearGrant] });
    fillRequired(d, { why: false });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(await within(d).findByTestId("save-problem")).toHaveTextContent(/Say why github is needed/);
    fillRequired(d);
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(clicked).toHaveLength(1);
    const raw = await zipText();
    expect(raw).toContain("app: github");
    expect(raw).toContain("level: act");
    expect(raw).toContain("tools:");
    expect(raw).toContain("- linear_issue_get");
    expect(raw).toContain("- linear_issue_create");
  });

  it("writes a library skill as a reference and says when one could not be read", async () => {
    const d = await ready();
    await fillRequired(d);
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    const raw = await zipText();
    expect(raw).toContain('library: "https://gitlab.example.org/agents/skills.git"');
    expect(raw).toContain("ref: v1.2");
    expect(raw).toContain("path: skills");
    expect(raw).not.toContain("Library body that is never written.");
    expect(raw).not.toContain("skills/incident-writeup/");
    cleanup();
    serve({ libraries: { lib1: { ...library, url: null } } });
    mount();
    const d2 = dialog();
    await waitFor(() => expect(within(d2).getByTestId("template-groups")).toBeInTheDocument());
    expect(within(d2).getByText(/The library skill “incident-writeup” could not be read/)).toBeInTheDocument();
  });

  it("counts the organization and workspace skills and the pending one-time schedules it leaves out", async () => {
    extraSchedules = [{ name: "Morning", prompt: "Check.", cron: "0 8 * * 1-5", timezone: "UTC", enabled: true }, { name: "Once", prompt: "Call.", run_at: "2026-12-01T09:00", timezone: "UTC", enabled: true }];
    const d = await ready({
      effective: [{ tier: "workspace", app: null, enabled: true, shadows: [], skill: { name: "deploy-checklist", tier: "workspace", body: "", description: "", files: [], enabled: true } }],
    });
    expect(within(d).getByText("1 organization or workspace skill is not included.")).toBeInTheDocument();
    expect(within(d).getByText(/one-time schedule.*not included/)).toBeInTheDocument();
    fillRequired(d);
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    const raw = await zipText();
    expect(raw).toContain('cron: "0 8 * * 1-5"');
    expect(raw).not.toContain("run_at");
  });

  it("refuses each field problem in one sentence", async () => {
    const d = await ready({ accounts: [githubAccount] });
    fillRequired(d);
    fireEvent.change(within(d).getByLabelText("Version"), { target: { value: "1.0" } });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(within(d).getByTestId("save-problem")).toHaveTextContent(/semver/);
    fireEvent.change(within(d).getByLabelText("Version"), { target: { value: "1.0.0" } });
    fireEvent.change(within(d).getByLabelText("Id"), { target: { value: "Bad Id" } });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(within(d).getByTestId("save-problem")).toHaveTextContent(/id is lowercase/);
    fireEvent.change(within(d).getByLabelText("Id"), { target: { value: "daneel" } });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(clicked).toHaveLength(1);
  });

  it("refuses a helper that is not UTF-8 text, executable or not, and a placeholder in the instructions", async () => {
    const d = await ready({ accounts: [githubAccount], helpers: [{ path: "run.bin", size: 4, executable: false, sha256: "x" }], helperBodies: { "run.bin": new Uint8Array([10, 0, 255, 254]) } });
    fillRequired(d);
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(await within(d).findByTestId("save-problem")).toHaveTextContent(/run\.bin.*not UTF-8 text/);
    // A placeholder in the instructions is refused, naming the file (through templateFiles).
    cleanup();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    seedMoreControls(qc, true);
    render(
      <QueryClientProvider client={qc}>
        <SaveTemplateDialog
          agent={{ id: "a1", name: "Daneel", handle: "daneel", brief: "Fill {input.jira_project}.", model_id: null, proactive_messaging: "ask", org_id: "org1", workspace_id: "ws1" } as never}
          onClose={() => undefined}
        />
      </QueryClientProvider>,
    );
    const d2 = dialog();
    await waitFor(() => expect(within(d2).getByTestId("template-groups")).toBeInTheDocument());
    fillRequired(d2);
    fireEvent.click(within(d2).getByRole("button", { name: "Save" }));
    expect(await within(d2).findByTestId("save-problem")).toHaveTextContent(/instructions\.md/);
  });

  it("shows a preview of the README through the shared Markdown", async () => {
    const d = await ready();
    fireEvent.click(within(d).getByRole("button", { name: "Preview" }));
    expect(within(d).getByTestId("readme-preview")).toBeInTheDocument();
  });
});
