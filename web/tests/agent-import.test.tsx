import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AgentExportImport } from "@/components/agents/export-import";
import { exportZip, type AgentExport } from "@/lib/export/agent-export";
import { DEFAULT_CHOICES, liveImportApi, runImport, summaryLine, type ImportApi, type ImportExisting } from "@/lib/export/agent-import";
import { seedMoreControls } from "./support/render";
import { json } from "./support/api";

// P5-REFINE-19 item 4: Import creates through the existing routes, skips what exists, pauses schedules and webhooks,
// shows a fresh secret once, and never changes access, accounts or the AI key.

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const exp: AgentExport = {
  format: 1,
  name: "Daneel",
  model: "default",
  may_message_first: false,
  instructions: "You are Daneel.",
  memories: [
    { key: "likes-tea", updated: "2026-11-01T09:00:00Z", content: "Ada likes tea." },
    { key: "timezone", updated: "2026-11-02T09:00:00Z", content: "Zurich" },
  ],
  skills: [{ name: "release-notes", useItWhen: "When asked what shipped", body: "# Notes", files: [{ path: "run.sh", content: new Uint8Array([1, 2, 3]) }] }],
  automations: [
    { kind: "schedule", name: "Morning", prompt: "Check.", cadence: { cron: "0 8 * * 1-5" }, time_zone: "Europe/Zurich", enabled: true },
    { kind: "webhook", name: "Deploy notice", instructions: "Say what shipped.", verification: "signed", enabled: true },
  ],
};
const none: ImportExisting = { instructions: "Mine.", memoryKeys: [], skillNames: [], scheduleNames: [], webhookNames: [] };

function fakeApi() {
  const calls: string[] = [];
  const io: ImportApi = {
    async setInstructions(t) { calls.push(`instructions:${t}`); },
    async createMemory(m) { calls.push(`memory:${m.key}`); },
    async createSkill(s) { calls.push(`skill:${s.name}`); return { id: "sk1" }; },
    async putSkillFile(id, p) { calls.push(`file:${id}/${p}`); },
    async createSchedule(a) { calls.push(`schedule:${a.name}`); return { id: "sc1" }; },
    async pauseSchedule(id) { calls.push(`pause-schedule:${id}`); },
    async createWebhook(a) { calls.push(`webhook:${a.name}`); return { id: "wh1", secret: "whsec_fresh" }; },
    async pauseWebhook(id) { calls.push(`pause-webhook:${id}`); },
  };
  return { calls, io };
}

describe("runImport", () => {
  it("creates everything ticked, pauses schedules and webhooks, and returns the new secret", async () => {
    const { calls, io } = fakeApi();
    const progress: string[] = [];
    const r = await runImport(exp, { ...DEFAULT_CHOICES, instructionsMode: "replace" }, none, io, (d, t) => progress.push(`${d}/${t}`));
    expect(calls).toEqual([
      "instructions:You are Daneel.",
      "memory:likes-tea",
      "memory:timezone",
      "skill:release-notes",
      "file:sk1/run.sh",
      "schedule:Morning",
      "pause-schedule:sc1",
      "webhook:Deploy notice",
      "pause-webhook:wh1",
    ]);
    expect(progress.at(-1)).toBe("6/6");
    expect(r.secrets).toEqual([{ name: "Deploy notice", secret: "whsec_fresh" }]);
    expect(r.schedulesPaused).toBe(1);
    expect(r.webhooksPaused).toBe(1);
    expect(r.failed).toEqual([]);
  });

  it("creates a one-time schedule from its run_at and time zone, paused like the others", async () => {
    const once: AgentExport = { ...exp, memories: [], skills: [], automations: [{ kind: "schedule", name: "Remind me", prompt: "Call Ada.", cadence: { preset: "once", run_at: "2026-12-01T09:00" }, time_zone: "Europe/Zurich", enabled: true }] };
    const posted: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request) => {
        const url = new URL(input.url, "http://localhost");
        if (input.method === "POST" && url.pathname === "/v1/agents/a1/schedules") {
          posted.push(JSON.parse(await input.clone().text()));
          return json(201, { id: "sc9" });
        }
        return json(200, {});
      }),
    );
    const r = await runImport(once, DEFAULT_CHOICES, none, liveImportApi("a1"));
    expect(posted).toEqual([{ name: "Remind me", prompt: "Call Ada.", run_at: "2026-12-01T09:00", timezone: "Europe/Zurich" }]);
    expect(r.schedulesPaused).toBe(1);
    expect(r.done).toEqual(["Schedule “Remind me” (paused)"]);
  });

  it("keeps the agent's own instructions by default", async () => {
    const { calls, io } = fakeApi();
    await runImport(exp, DEFAULT_CHOICES, none, io);
    expect(calls.some((c) => c.startsWith("instructions"))).toBe(false);
  });

  it("skips what exists, by name, and says so", async () => {
    const { calls, io } = fakeApi();
    const r = await runImport(exp, DEFAULT_CHOICES, { ...none, memoryKeys: ["likes-tea"], skillNames: ["release-notes"], scheduleNames: ["Morning"], webhookNames: ["Deploy notice"] }, io);
    expect(calls).toEqual(["memory:timezone"]);
    expect(r.skipped).toEqual([
      "Skipped: memory “likes-tea” already exists",
      "Skipped: skill “release-notes” already exists",
      "Skipped: schedule “Morning” already exists",
      "Skipped: webhook “Deploy notice” already exists",
    ]);
  });

  it("imports only the groups ticked", async () => {
    const { calls, io } = fakeApi();
    await runImport(exp, { ...DEFAULT_CHOICES, memories: false, skills: false, webhooks: false }, none, io);
    expect(calls).toEqual(["schedule:Morning", "pause-schedule:sc1"]);
  });

  it("lists a failure and goes on with the rest", async () => {
    const { calls, io } = fakeApi();
    io.createMemory = async (m) => {
      if (m.key === "likes-tea") throw new Error("boom");
      calls.push(`memory:${m.key}`);
    };
    const r = await runImport(exp, DEFAULT_CHOICES, none, io);
    expect(r.failed).toHaveLength(1);
    expect(r.failed[0]).toContain("Memory “likes-tea”");
    expect(calls).toContain("memory:timezone");
    expect(calls).toContain("skill:release-notes");
  });

  it("imports a locked memory locked (P6-MEMLOCK, C-1150)", async () => {
    const { calls, io } = fakeApi();
    const lockedExp: AgentExport = {
      ...exp,
      memories: [{ key: "pinned", updated: "2026-11-01T09:00:00Z", content: "Rules.", locked: true }, ...exp.memories.slice(1)],
    };
    const seen: (boolean | undefined)[] = [];
    const spy = io.createMemory;
    io.createMemory = async (m) => {
      seen.push(m.locked);
      return spy(m);
    };
    await runImport(lockedExp, DEFAULT_CHOICES, none, io);
    expect(calls).toEqual(["memory:pinned", "memory:timezone", "skill:release-notes", "file:sk1/run.sh", "schedule:Morning", "pause-schedule:sc1", "webhook:Deploy notice", "pause-webhook:wh1"]);
    expect(seen).toEqual([true, undefined]);
  });

  it("writes the checklist line", () => {
    expect(summaryLine(exp)).toBe("Instructions · 2 memories · 1 skill · 1 schedule · 1 webhook");
    // (P6-MEMLOCK, C-1150) The locked count shows only when any memory is locked.
    expect(summaryLine({ ...exp, memories: [{ ...exp.memories[0]!, locked: true }] })).toBe(
      "Instructions · 1 memory (1 locked) · 1 skill · 1 schedule · 1 webhook",
    );
    expect(summaryLine({ ...exp, memories: [] })).toBe("Instructions · 1 skill · 1 schedule · 1 webhook");
  });
});

// ---- the screens ---------------------------------------------------------------------------------------------------

interface Call {
  method: string;
  path: string;
  body: unknown;
}

function serve(extra: { memories?: unknown[]; skills?: unknown[]; schedules?: unknown[]; webhooks?: unknown[] } = {}) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const path = new URL(req.url).pathname;
      let body: unknown = null;
      if (req.method !== "GET") body = await req.clone().json().catch(() => null);
      calls.push({ method: req.method, path, body });
      const key = `${req.method} ${path}`;
      if (key === "GET /v1/agents/a1/memories") return json(200, { items: extra.memories ?? [], next_cursor: null });
      if (key === "GET /v1/agents/a1/skills") return json(200, { items: extra.skills ?? [], next_cursor: null });
      if (key === "GET /v1/agents/a1/schedules") return json(200, { items: extra.schedules ?? [], next_cursor: null });
      if (key === "GET /v1/agents/a1/webhooks") return json(200, { items: extra.webhooks ?? [], next_cursor: null });
      if (key === "POST /v1/agents/a1/memories") return json(201, { key: "x" });
      if (key === "POST /v1/agents/a1/skills") return json(201, { id: "sk1", name: "release-notes" });
      if (key === "PUT /v1/skills/sk1/files/run.sh") return json(200, { id: "sk1" });
      if (key === "POST /v1/agents/a1/schedules") return json(201, { id: "sc1" });
      if (key === "PATCH /v1/schedules/sc1") return json(200, { id: "sc1" });
      if (key === "POST /v1/agents/a1/webhooks") return json(201, { webhook: { id: "wh1" }, url: "https://aurora.test/hooks/TOKEN", secret: "whsec_fresh" });
      if (key === "PATCH /v1/webhooks/wh1") return json(200, { id: "wh1" });
      if (key === "PATCH /v1/agents/a1") return json(200, { id: "a1" });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

const agent = { id: "a1", name: "Rex", handle: "rex", brief: "Mine.", model_id: null, proactive_messaging: "ask" } as never;

function mount(more = true) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seedMoreControls(qc, more);
  return render(
    <QueryClientProvider client={qc}>
      <AgentExportImport agent={agent} />
    </QueryClientProvider>,
  );
}

function file(bytes: Uint8Array, name = "daneel.zip") {
  const f = new File([bytes as BlobPart], name, { type: "application/zip" });
  // jsdom's File may lack arrayBuffer.
  if (typeof f.arrayBuffer !== "function") Object.defineProperty(f, "arrayBuffer", { value: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
  return f;
}

describe("Export and import on the Settings page", () => {
  beforeEach(() => {
    URL.createObjectURL = vi.fn(() => "blob:x");
    URL.revokeObjectURL = vi.fn();
  });

  it("is Expert: nothing with Show more controls off", () => {
    serve();
    mount(false);
    expect(screen.queryByRole("button", { name: /Export Rex/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Import into Rex/ })).toBeNull();
  });

  it("exports a zip named <handle>-<date>.zip and says what is in it", async () => {
    serve({
      memories: [{ key: "likes-tea", content: "tea", updated_at: "2026-11-01T09:00:00Z" }],
      schedules: [{ name: "Morning", prompt: "Check.", cron: "0 8 * * *", timezone: "UTC", enabled: true }],
    });
    const blobs: Blob[] = [];
    URL.createObjectURL = vi.fn((b: Blob | MediaSource) => (blobs.push(b as Blob), "blob:x"));
    const clicked: string[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      clicked.push(this.download);
    });
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Export Rex…" }));
    const dialog = await screen.findByRole("dialog", { name: "Export Rex" });
    expect(await within(dialog).findByTestId("export-summary")).toHaveTextContent("Instructions · 1 memory · 1 schedule");
    fireEvent.click(within(dialog).getByRole("button", { name: "Download zip" }));
    expect(clicked).toHaveLength(1);
    expect(clicked[0]).toMatch(/^rex-\d{4}-\d{2}-\d{2}\.zip$/);
    expect(blobs).toHaveLength(1);
    expect(blobs[0]!.type).toBe("application/zip");
  });

  it("imports a zip: checklist, Keep mine by default, then creates paused items and shows the secret once", async () => {
    const calls = serve({ memories: [{ key: "timezone" }] });
    const bytes = exportZip(exp, "daneel").bytes;
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Import into Rex…" }));
    const dialog = await screen.findByRole("dialog", { name: "Import into Rex" });
    fireEvent.change(within(dialog).getByLabelText("Aurora export (.zip)"), { target: { files: [file(bytes)] } });
    expect(await within(dialog).findByTestId("import-summary")).toHaveTextContent("Instructions · 2 memories · 1 skill · 1 schedule · 1 webhook");
    expect(within(dialog).getByRole("radio", { name: "Keep mine" })).toBeChecked();
    expect(within(dialog).getByText("Created paused. Check the schedule, then turn it on.")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Import" }));
    expect(await within(dialog).findByTestId("import-report")).toBeInTheDocument();
    const writes = calls.filter((c) => c.method !== "GET").map((c) => `${c.method} ${c.path}`);
    expect(writes).toEqual([
      "POST /v1/agents/a1/memories",
      "POST /v1/agents/a1/skills",
      "PUT /v1/skills/sk1/files/run.sh",
      "POST /v1/agents/a1/schedules",
      "PATCH /v1/schedules/sc1",
      "POST /v1/agents/a1/webhooks",
      "PATCH /v1/webhooks/wh1",
    ]);
    expect(calls.find((c) => c.path === "/v1/agents/a1/memories" && c.method === "POST")!.body).toMatchObject({ key: "likes-tea" });
    // Paused, and no access, account or AI-key route was touched.
    expect(calls.find((c) => c.path === "/v1/schedules/sc1")!.body).toEqual({ enabled: false });
    expect(calls.find((c) => c.path === "/v1/webhooks/wh1")!.body).toEqual({ enabled: false });
    expect(calls.some((c) => /access|grant|account|key|byo/.test(c.path))).toBe(false);
    expect(within(dialog).getByText("Skipped: memory “timezone” already exists")).toBeInTheDocument();
    // The new secret, once.
    expect(await screen.findByTestId("secret-key")).toHaveTextContent("whsec_fresh");
    expect(document.body.textContent).not.toContain("TOKEN");
  });

  it("replaces instructions only when asked", async () => {
    const calls = serve();
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Import into Rex…" }));
    const dialog = await screen.findByRole("dialog", { name: "Import into Rex" });
    fireEvent.change(within(dialog).getByLabelText("Aurora export (.zip)"), { target: { files: [file(exportZip(exp, "daneel").bytes)] } });
    await within(dialog).findByTestId("import-summary");
    fireEvent.click(within(dialog).getByRole("radio", { name: "Replace instructions" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Import" }));
    await within(dialog).findByTestId("import-report");
    expect(calls.find((c) => c.method === "PATCH" && c.path === "/v1/agents/a1")!.body).toEqual({ brief: "You are Daneel." });
  });

  it("refuses a file that is not an export, in one sentence", async () => {
    serve();
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Import into Rex…" }));
    const dialog = await screen.findByRole("dialog", { name: "Import into Rex" });
    fireEvent.change(within(dialog).getByLabelText("Aurora export (.zip)"), { target: { files: [file(new TextEncoder().encode("hello there, this is not a zip"), "x.zip")] } });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("This is not a zip file.");
    expect(within(dialog).getByRole("button", { name: "Import" })).toBeDisabled();
  });

  it("round trip: export an agent, import it into another, nothing secret in between", async () => {
    // The first agent's data, read through the same routes the export uses.
    serve({
      memories: [{ key: "likes-tea", content: "tea", updated_at: "2026-11-01T09:00:00Z" }, { key: "timezone", content: "Zurich", updated_at: "2026-11-02T09:00:00Z" }],
      skills: [{ id: "s", name: "release-notes", description: "When asked", body: "# Notes", files: [] }],
      schedules: [{ name: "Morning", prompt: "Check.", cron: "0 8 * * *", timezone: "UTC", enabled: true }],
      webhooks: [{ name: "Deploy", instructions: "Say.", url_prefix: "https://aurora.test/hooks/PREFIX", verification: { kind: "hmac_sha256" }, enabled: true }],
    });
    const blobs: Blob[] = [];
    URL.createObjectURL = vi.fn((b: Blob | MediaSource) => (blobs.push(b as Blob), "blob:x"));
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => undefined);
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Export Rex…" }));
    const ex = await screen.findByRole("dialog", { name: "Export Rex" });
    await within(ex).findByTestId("export-summary");
    fireEvent.click(within(ex).getByRole("button", { name: "Download zip" }));
    const bytes = await new Promise<Uint8Array>((resolve) => {
      const r = new FileReader();
      r.onload = () => resolve(new Uint8Array(r.result as ArrayBuffer));
      r.readAsArrayBuffer(blobs[0]!);
    });
    const raw = new TextDecoder().decode(bytes);
    for (const leak of ["PREFIX", "https://", "hooks/"]) expect(raw).not.toContain(leak);
    cleanup();
    // Into another agent that has nothing.
    const calls = serve();
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Import into Rex…" }));
    const dialog = await screen.findByRole("dialog", { name: "Import into Rex" });
    fireEvent.change(within(dialog).getByLabelText("Aurora export (.zip)"), { target: { files: [file(bytes, "rex.zip")] } });
    expect(await within(dialog).findByTestId("import-summary")).toHaveTextContent("Instructions · 2 memories · 1 skill · 1 schedule · 1 webhook");
    fireEvent.click(within(dialog).getByRole("button", { name: "Import" }));
    await waitFor(() => expect(within(dialog).getByTestId("import-report")).toBeInTheDocument());
    const writes = calls.filter((c) => c.method !== "GET").map((c) => `${c.method} ${c.path}`);
    expect(writes.filter((w) => w === "POST /v1/agents/a1/memories")).toHaveLength(2);
    expect(writes).toContain("POST /v1/agents/a1/skills");
    expect(writes).toContain("PATCH /v1/schedules/sc1");
    expect(writes).toContain("PATCH /v1/webhooks/wh1");
    expect(await screen.findByTestId("secret-key")).toHaveTextContent("whsec_fresh");
  });

  it("exports a locked memory and imports it locked (P6-MEMLOCK, C-1150)", async () => {
    serve({ memories: [{ key: "pinned", content: "Rules.", updated_at: "2026-11-01T09:00:00Z", locked: true }] });
    const blobs: Blob[] = [];
    URL.createObjectURL = vi.fn((b: Blob | MediaSource) => (blobs.push(b as Blob), "blob:x"));
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => undefined);
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Export Rex…" }));
    const ex = await screen.findByRole("dialog", { name: "Export Rex" });
    await within(ex).findByTestId("export-summary");
    fireEvent.click(within(ex).getByRole("button", { name: "Download zip" }));
    const bytes = await new Promise<Uint8Array>((resolve) => {
      const r = new FileReader();
      r.onload = () => resolve(new Uint8Array(r.result as ArrayBuffer));
      r.readAsArrayBuffer(blobs[0]!);
    });
    // The zip carries the lock; the exported yaml says `locked: true`.
    expect(new TextDecoder().decode(bytes)).toContain("locked: true");
    cleanup();
    const calls = serve();
    mount();
    fireEvent.click(screen.getByRole("button", { name: "Import into Rex…" }));
    const dialog = await screen.findByRole("dialog", { name: "Import into Rex" });
    fireEvent.change(within(dialog).getByLabelText("Aurora export (.zip)"), { target: { files: [file(bytes, "rex.zip")] } });
    expect(await within(dialog).findByTestId("import-summary")).toHaveTextContent("1 memory (1 locked)");
    fireEvent.click(within(dialog).getByRole("button", { name: "Import" }));
    await within(dialog).findByTestId("import-report");
    expect(calls.find((c) => c.method === "POST" && c.path === "/v1/agents/a1/memories")!.body).toMatchObject({
      key: "pinned",
      locked: true,
    });
  });
});
