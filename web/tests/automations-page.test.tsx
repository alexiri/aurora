import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AutomationsTab } from "@/components/automations/automations-tab";
import { runThreadName } from "@/components/automations/schedule-row";
import { dismissUndoToast, UndoToastHost } from "@/components/ui/undo-toast";
import { setDisplayTimeZone } from "@/lib/format-date";
import { json, err, type Handler } from "./support/api";
import { seedMoreControls } from "./support/render";

// (P5-REFINE-14, D-405) The Automations page: one list of schedules, webhooks and chats, presets from every 5 minutes, runs and
// deliveries in words with links to their own threads, dialogs for rotate / new secret / delete, Pause with Undo.

afterEach(() => {
  cleanup();
  // The toast is one module-level slot: the next test must not see this test's.
  dismissUndoToast();
  vi.unstubAllGlobals();
});

type Call = { method: string; path: string; search: string; body: unknown };
let qcRef: QueryClient;

function mockApi(routes: Record<string, Handler>) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      calls.push({ method: req.method, path: url.pathname, search: url.search, body: text ? JSON.parse(text) : null });
      const h = routes[`${req.method} ${decodeURIComponent(url.pathname)}`];
      if (h) return h(req);
      if (url.pathname.endsWith("/presences") || url.pathname.endsWith("/presence-options")) return json(200, { items: [], next_cursor: null });
      if (url.pathname === "/v1/connectors") return json(200, { items: [{ id: "mattermost", display_name: "Mattermost" }, { id: "slack", display_name: "Slack" }] });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

/** The create call (the preview is a POST too). */
const created = (calls: Call[]) => calls.find((c) => c.method === "POST" && c.path === "/v1/agents/a1/schedules");

function mount(props: Partial<React.ComponentProps<typeof AutomationsTab>> = {}, more = true) {
  qcRef = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seedMoreControls(qcRef, more);
  return render(
    <QueryClientProvider client={qcRef}>
      <AutomationsTab agentId="a1" agentName="Daneel" agentBase="/o/acme/w/main/agents/daneel" wsId="w1" orgId="o1" canOwn defaultTimezone="Europe/Zurich" workspaceName="Acme workspace" {...props} />
      <UndoToastHost />
    </QueryClientProvider>,
  );
}

const sched = (over: Record<string, unknown> = {}) => ({
  id: "s1", agent_id: "a1", consecutive_failures: 0, conversation_hidden: false, created_at: "2026-01-01T00:00:00Z", cron: "0 9 * * *", enabled: true,
  name: "Morning summary", prompt: "Say hi", timezone: "Europe/Zurich", updated_at: "2026-01-01T00:00:00Z", anchor: "creator", next_runs: [], ...over,
});
const hook = (over: Record<string, unknown> = {}) => ({
  id: "w1", agent_id: "a1", created_at: "2026-01-01T00:00:00Z", enabled: true, instructions: "Handle it", name: "Deploy notice", anchor: "creator",
  updated_at: "2026-01-01T00:00:00Z", url_prefix: "https://aurora.example/hooks/whk_ab12", verification: { kind: "hmac_sha256", header: "X-Signature", format: "sha256=<hex>" }, ...over,
});
const run = (id: string, over: Record<string, unknown> = {}) => ({
  id, schedule_id: "s1", fired_at: "2026-01-02T09:00:00Z", scheduled_for: "2026-01-02T09:00:00Z", created_at: "2026-01-02T09:00:00Z", outcome: "fired", result: "answered", reason: null, conversation_id: "c-" + id, ...over,
});
const page = (items: unknown[]) => () => json(200, { items, next_cursor: null });

async function openMenu(name: string, item: string) {
  const trigger = await screen.findByRole("button", { name: `More for ${name}` });
  fireEvent.keyDown(trigger, { key: "Enter" });
  fireEvent.click(await screen.findByRole("menuitem", { name: item }));
}

describe("the page", () => {
  it("frames it: title, subtitle, one header button, an empty state with the same button", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([]) });
    mount();
    expect(await screen.findByRole("heading", { name: "Automations" })).toBeInTheDocument();
    expect(screen.getByText("When Daneel runs: on a schedule, when something calls its address, or in a chat.")).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "No automations yet" })).toBeInTheDocument();
    expect(screen.getByText("Daneel can run on a schedule, when something calls it, or in a chat.")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "New automation" }).length).toBeGreaterThan(0);
  });

  it("lists schedules and webhooks in one list, with counts in the filter, and filters by kind", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([sched()]), "GET /v1/agents/a1/webhooks": page([hook()]) });
    mount();
    await screen.findByText("Morning summary");
    expect(screen.getByText("Deploy notice")).toBeInTheDocument();
    expect(screen.getAllByRole("list", { name: "Automations" })).toHaveLength(1);
    const filter = screen.getByRole("radiogroup", { name: "Show" });
    expect(within(filter).getByRole("radio", { name: "All 2" })).toBeChecked();
    fireEvent.click(within(filter).getByRole("radio", { name: "Webhooks 1" }));
    expect(screen.queryByText("Morning summary")).toBeNull();
    expect(screen.getByText("Deploy notice")).toBeInTheDocument();
    fireEvent.click(within(filter).getByRole("radio", { name: "Schedules 1" }));
    expect(screen.getByText("Morning summary")).toBeInTheDocument();
    expect(screen.queryByText("Deploy notice")).toBeNull();
  });

  it("the old ?tab ids open it filtered to their kind", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([sched()]), "GET /v1/agents/a1/webhooks": page([hook()]) });
    mount({ initialFilter: "webhooks" });
    await screen.findByText("Deploy notice");
    expect(screen.queryByText("Morning summary")).toBeNull();
    expect(screen.getByRole("radio", { name: "Webhooks 1" })).toBeChecked();
  });

  it("New automation asks the kind first, in one sentence each", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([]) });
    mount();
    fireEvent.click((await screen.findAllByRole("button", { name: "New automation" }))[0]!);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Daneel runs at set times.")).toBeInTheDocument();
    expect(within(dialog).getByText("An outside system sends Daneel a request.")).toBeInTheDocument();
    // No chat platform: the empty state with the ask (and its optional reason) instead of a choice (channels-3).
    expect(within(dialog).getByText("Daneel isn't in any chat yet")).toBeInTheDocument();
    expect(within(dialog).getByText("Only organization owners and admins can connect a chat platform.")).toBeInTheDocument();
    expect(within(dialog).getByLabelText(/Why do you need it\?/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Send request" })).toBeInTheDocument();
    expect(dialog).not.toHaveTextContent("Without one");
    fireEvent.click(within(dialog).getByRole("button", { name: /On a schedule/ }));
    expect(await screen.findByRole("form", { name: "New schedule" })).toBeInTheDocument();
  });

  it("states are On and Paused and nothing else: no Enabled, Disabled, enum value, cron or id on the page", async () => {
    mockApi({
      "GET /v1/agents/a1/schedules": page([sched({ cron: "*/5 * * * *", enabled: false, disabled_reason: "user", last_fired_at: "2026-01-02T09:00:00Z", last_result: "skipped_by_gate" }), sched({ id: "s2", name: "Weekdays", cron: "30 8 * * 1-5" })]),
      "GET /v1/agents/a1/webhooks": page([hook({ enabled: false }), hook({ id: "w2", name: "Open one", verification: { kind: "none" } })]),
    });
    mount();
    await screen.findByText("Weekdays");
    const text = document.body.textContent ?? "";
    expect(screen.getAllByText("Paused").length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByText("On").length).toBeGreaterThanOrEqual(2);
    for (const bad of ["Enabled", "Disabled", "hmac_sha256", "rejected_signature", "*/5 * * * *", "30 8 * * 1-5", "s1", "w1"]) expect(text, bad).not.toContain(bad);
    expect(text).not.toMatch(/\bnone\b/);
    expect(text).not.toMatch(/[0-9a-f]{8}-[0-9a-f]{4}-/);
    expect(text).toContain("Every 5 minutes · paused, won't run until resumed");
    expect(text).toContain("Every weekday at 08:30 · Zurich");
  });
});

describe("a schedule row", () => {
  it("Run now says Started with a link to the run's own thread", async () => {
    mockApi({
      "GET /v1/agents/a1/schedules": page([sched()]),
      "GET /v1/agents/a1/webhooks": page([]),
      "POST /v1/schedules/s1/run-now": () => json(201, run("r9", { result: "running", conversation_id: "conv-9" })),
    });
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Run now" }));
    const status = await screen.findByTestId("run-started");
    expect(status).toHaveTextContent("Started · Open the conversation");
    expect(within(status).getByRole("link", { name: "Open the conversation" })).toHaveAttribute("href", "/o/acme/w/main/agents/daneel?tab=chat&c=conv-9");
    // (schedules-3) The same words as a toast, at every width, linking the run's own thread.
    const toast = await screen.findByTestId("undo-toast");
    expect(toast).toHaveTextContent("Started · Open the conversation");
    expect(within(toast).getByRole("link", { name: "Open the conversation" })).toHaveAttribute("href", "/o/acme/w/main/agents/daneel?tab=chat&c=conv-9");
  });

  it("Run now skipped by its gate says so in a toast without a link", async () => {
    mockApi({
      "GET /v1/agents/a1/schedules": page([sched()]),
      "GET /v1/agents/a1/webhooks": page([]),
      "POST /v1/schedules/s1/run-now": () => json(201, run("r9", { result: "skipped_by_gate", conversation_id: null })),
    });
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Run now" }));
    const toast = await screen.findByTestId("undo-toast");
    expect(toast).toHaveTextContent("Skipped by the gate · nothing to report");
    expect(within(toast).queryByRole("link")).toBeNull();
  });

  it("a person's pause says who paused it; a pause that is not a person's says nothing about a name (C-1096)", async () => {
    mockApi({
      "GET /v1/agents/a1/schedules": page([
        sched({ enabled: false, disabled_reason: "user", paused_by: { id: "u2", display_name: "Carol Member" } }),
        sched({ id: "s2", name: "Auto-paused", enabled: false, disabled_reason: "failures", paused_by: null }),
      ]),
      "GET /v1/agents/a1/webhooks": page([hook({ enabled: false, disabled_reason: "user", paused_by: { id: "u3", display_name: "Dana Admin" } })]),
    });
    mount();
    await screen.findByText("Morning summary");
    const text = document.body.textContent ?? "";
    expect(text).toContain("Paused by Carol Member");
    expect(text).toContain("Paused by Dana Admin");
    expect(text.match(/Paused by/g)).toHaveLength(2);
    // The name, the dot and the next words are separated: no "Paused by Carol Member ·Last run" (batch 10 item 3).
    expect(text).toMatch(/Paused by Carol Member · Last run: /);
    expect(text).toMatch(/Paused by Dana Admin · Last delivery: /);
    expect(text).not.toMatch(/·(Last|Never|\w)/);
  });

  it("Run now maps schedule_busy to words", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([sched()]), "GET /v1/agents/a1/webhooks": page([]), "POST /v1/schedules/s1/run-now": () => err(409, "schedule_busy", "busy") });
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Run now" }));
    expect((await screen.findByRole("alert")).textContent).toContain("already has a run in progress");
  });

  it("Runs is a table: When, Outcome in words, and a link to each run's thread", async () => {
    mockApi({
      "GET /v1/agents/a1/schedules": page([sched()]),
      "GET /v1/agents/a1/webhooks": page([]),
      "GET /v1/schedules/s1/runs": page([
        run("r1"),
        run("r2", { outcome: "gated_quiet", result: "skipped_by_gate", conversation_id: null }),
        run("r3", { result: "failed", reason: "gate_failed" }),
        run("r4", { result: "running" }),
        run("r5", { result: "failed", reason: "computer_failed", conversation_id: null, turn_id: "t-5" }),
      ]),
    });
    mount();
    await screen.findByText("Morning summary");
    await openMenu("Morning summary", "Runs");
    const table = await screen.findByRole("table", { name: "Runs" });
    expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["When", "Outcome", "Conversation"]);
    expect(within(table).getByText("Answered")).toBeInTheDocument();
    expect(within(table).getByText("Skipped by the gate · nothing to report")).toBeInTheDocument();
    expect(within(table).getByText("Failed · the check could not run")).toBeInTheDocument();
    expect(within(table).getByText("Running")).toBeInTheDocument();
    // (schedules-3) The link reads like the run thread's own name: "<Schedule name> · <time>".
    const threadName = runThreadName("Morning summary", "2026-01-02T09:00:00Z");
    expect(threadName).toMatch(/^Morning summary · \d{1,2} \w{3}( \d{4})? \d{2}:\d{2}$/);
    const links = within(table).getAllByRole("link", { name: threadName });
    expect(links.map((l) => l.getAttribute("href"))).toEqual([
      "/o/acme/w/main/agents/daneel?tab=chat&c=c-r1",
      "/o/acme/w/main/agents/daneel?tab=chat&c=c-r3",
      "/o/acme/w/main/agents/daneel?tab=chat&c=c-r4",
    ]);
    expect(within(table).queryByRole("link", { name: "Open the conversation" })).toBeNull();
    // A skipped run has no thread: None. A failed run without a thread offers Open details.
    expect(within(table).getByText("None")).toBeInTheDocument();
    fireEvent.click(within(table).getByRole("button", { name: "Open details" }));
    const dialog = await screen.findByRole("dialog", { name: "What happened to this run" });
    expect(dialog).toHaveTextContent("Failed");
    // The raw codes sit under Technical details only.
    const tech = within(dialog).getByText("Technical details").closest("details")!;
    expect(tech).toHaveTextContent("reason: computer_failed");
    expect(tech).toHaveTextContent("turn: t-5");
    expect(tech.open).toBe(false);
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "What happened to this run" })).toBeNull());
  });

  it("pages the run history with Load more, following next_cursor", async () => {
    const cursors: (string | null)[] = [];
    mockApi({
      "GET /v1/agents/a1/schedules": page([sched()]),
      "GET /v1/agents/a1/webhooks": page([]),
      "GET /v1/schedules/s1/runs": (r) => {
        const c = new URL(r.url, "http://localhost").searchParams.get("cursor");
        cursors.push(c);
        return c ? json(200, { items: [run("r2", { result: "skipped", reason: "missed" })], next_cursor: null }) : json(200, { items: [run("r1", { outcome: "gated_quiet", result: "skipped_by_gate" })], next_cursor: "CUR1" });
      },
    });
    mount();
    await screen.findByText("Morning summary");
    await openMenu("Morning summary", "Runs");
    expect(await screen.findByText("Skipped by the gate · nothing to report")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    expect(await screen.findByText("Skipped · it was already running")).toBeInTheDocument();
    expect(cursors).toEqual([null, "CUR1"]);
  });

  it("Pause sends enabled=false with no confirmation and a toast whose Undo resumes", async () => {
    const calls = mockApi({
      "GET /v1/agents/a1/schedules": page([sched()]),
      "GET /v1/agents/a1/webhooks": page([]),
      "PATCH /v1/schedules/s1": () => json(200, sched({ enabled: false })),
    });
    mount();
    await screen.findByText("Morning summary");
    await openMenu("Morning summary", "Pause");
    await waitFor(() => expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ enabled: false }));
    expect(screen.queryByRole("dialog")).toBeNull();
    const toast = await screen.findByText("Paused Morning summary");
    expect(toast).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "PATCH").map((c) => c.body)).toEqual([{ enabled: false }, { enabled: true }]));
  });

  it("Delete says what happens and Cancel changes nothing", async () => {
    const calls = mockApi({ "GET /v1/agents/a1/schedules": page([sched()]), "GET /v1/agents/a1/webhooks": page([]), "DELETE /v1/schedules/s1": () => json(204, null) });
    mount();
    await screen.findByText("Morning summary");
    await openMenu("Morning summary", "Delete…");
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Delete Morning summary?");
    expect(dialog).toHaveTextContent("It stops running. Its past conversations are kept.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
  });

  it("Reassign opens as a dialog; members cannot reassign or keep it running", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([sched()]), "GET /v1/agents/a1/webhooks": page([]), "GET /v1/workspaces/w1/access": page([{ user_id: "u2", display_name: "Bob", email: "b@x.io", implicit: false, role: "member" }]) });
    mount({ canOwn: false });
    await screen.findByText("Morning summary");
    const trigger = screen.getByRole("button", { name: "More for Morning summary" });
    fireEvent.keyDown(trigger, { key: "Enter" });
    await screen.findByRole("menuitem", { name: "Pause" });
    expect(screen.queryByRole("menuitem", { name: "Reassign…" })).toBeNull();
    expect(screen.queryByText("Keep running without you")).toBeNull();
  });

  it("Edit opens in place on the preset the cron reads back as", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([sched({ cron: "30 8 * * 1-5" })]), "GET /v1/agents/a1/webhooks": page([]) });
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    const form = await screen.findByRole("form", { name: "Edit schedule" });
    expect(within(form).getByRole("radio", { name: "Weekdays" })).toBeChecked();
    expect(within(form).getByRole("combobox", { name: "Time of day" })).toHaveValue("08:30");
    expect(screen.queryByText("Morning summary")).toBeNull();
  });
});

describe("the schedule form", () => {
  async function openForm() {
    fireEvent.click((await screen.findAllByRole("button", { name: "New automation" }))[0]!);
    fireEvent.click(await screen.findByRole("button", { name: /On a schedule/ }));
    return screen.findByRole("form", { name: "New schedule" });
  }

  it("offers presets from Every 5 minutes (none shorter), the workspace's time zone, and builds the cron", async () => {
    const calls = mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([]), "POST /v1/agents/a1/schedules": () => json(201, sched()) });
    mount();
    const form = await openForm();
    expect(within(form).getAllByRole("radio").map((r) => (r as HTMLInputElement).parentElement?.textContent)).toEqual(["Every 5 minutes", "Hourly", "Every day", "Weekdays", "Weekly", "Every year", "Once", "Custom"]);
    expect(within(form).queryByRole("radio", { name: /minute$/ })).toBeNull();
    expect(within(form).getByRole("radio", { name: "Every day" })).toBeChecked();
    // (send-back) The time zone is a searchable select: the workspace's zone first, then type to narrow.
    const zone = within(form).getByRole("combobox", { name: "Time zone" });
    expect(zone.tagName).toBe("INPUT");
    expect(zone).toHaveValue("Europe/Zurich · Acme workspace");
    fireEvent.focus(zone);
    expect(within(within(form).getByRole("listbox")).getAllByRole("option")[0]).toHaveTextContent("Europe/Zurich · Acme workspace");
    fireEvent.change(zone, { target: { value: "tokyo" } });
    expect(within(within(form).getByRole("listbox")).getAllByRole("option").map((o) => o.textContent)).toEqual(["Asia/Tokyo"]);
    fireEvent.keyDown(zone, { key: "Enter" });
    expect(zone).toHaveValue("Asia/Tokyo");
    fireEvent.focus(zone);
    fireEvent.change(zone, { target: { value: "zurich" } });
    fireEvent.mouseDown(within(within(form).getByRole("listbox")).getByRole("option", { name: "Europe/Zurich · Acme workspace" }));
    expect(zone).toHaveValue("Europe/Zurich · Acme workspace");
    expect(within(form).getByRole("combobox", { name: "Time of day" })).toHaveValue("09:00");
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "Morning summary" } });
    fireEvent.change(within(form).getByLabelText("Prompt"), { target: { value: "go" } });
    fireEvent.click(within(form).getByRole("radio", { name: "Weekly" }));
    fireEvent.change(within(form).getByRole("combobox", { name: "Day of the week" }), { target: { value: "5" } });
    fireEvent.change(within(form).getByRole("combobox", { name: "Time of day" }), { target: { value: "18:30" } });
    expect(within(form).getByTestId("schedule-summary")).toHaveTextContent("Every Friday at 18:30 · Zurich");
    expect(within(form).getByTestId("schedule-summary")).toHaveTextContent("Schedules run at most every 5 minutes.");
    fireEvent.click(within(form).getByRole("button", { name: "Create schedule" }));
    await waitFor(() => expect(created(calls)?.body).toMatchObject({ name: "Morning summary", cron: "30 18 * * 5", timezone: "Europe/Zurich" }));
  });

  it("Custom flags a too-frequent cron as you type, before submit, and does not send it", async () => {
    const calls = mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([]), "POST /v1/agents/a1/schedules": () => json(201, sched()) });
    mount();
    const form = await openForm();
    fireEvent.click(within(form).getByRole("radio", { name: "Custom" }));
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "Fast" } });
    fireEvent.change(within(form).getByLabelText("Prompt"), { target: { value: "go" } });
    const cron = within(form).getByLabelText(/^Cron/);
    fireEvent.change(cron, { target: { value: "* * * * *" } });
    expect(await within(form).findByText("Too often: a schedule can run at most every 5 minutes. Try */5 * * * *.")).toBeInTheDocument();
    expect(within(form).getByRole("button", { name: "Create schedule" })).toBeDisabled();
    fireEvent.submit(form);
    expect(created(calls)).toBeUndefined();
    fireEvent.change(cron, { target: { value: "*/10 * * * *" } });
    expect(within(form).queryByText(/Too often/)).toBeNull();
    fireEvent.click(within(form).getByRole("button", { name: "Create schedule" }));
    await waitFor(() => expect(created(calls)?.body).toMatchObject({ cron: "*/10 * * * *" }));
  });

  it("Custom needs more controls: without the switch it is disabled with Show", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([]) });
    mount({}, false);
    const form = await openForm();
    expect(within(form).getByRole("radio", { name: "Custom" })).toBeDisabled();
    expect(within(form).getByText(/Custom needs more controls/)).toBeInTheDocument();
  });

  it("Every 5 minutes says 'Every 5 minutes · next 23:55, 00:00, 00:05.' from the preview (schedules-2)", async () => {
    mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([]),
      "POST /v1/agents/a1/schedules/preview": () => json(200, { next: ["2026-11-20T22:55:00Z", "2026-11-20T23:00:00Z", "2026-11-20T23:05:00Z"], interval_secs: 300, too_frequent: false, min_interval_secs: 300 }),
    });
    mount();
    const form = await openForm();
    fireEvent.click(within(form).getByRole("radio", { name: "Every 5 minutes" }));
    await waitFor(() => expect(within(form).getByTestId("schedule-summary")).toHaveTextContent(/^Every 5 minutes · next \d\d:\d\d, \d\d:\d\d, \d\d:\d\d\. Schedules run at most every 5 minutes\.$/), { timeout: 3000 });
  });

  it("previews the next runs through the API at once for a preset; a preset seen again comes from the cache", async () => {
    const calls = mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([]),
      "POST /v1/agents/a1/schedules/preview": () => json(200, { next: ["2026-11-20T08:00:00Z", "2026-11-21T08:00:00Z", "2026-11-22T08:00:00Z"], interval_secs: 86400, too_frequent: false, min_interval_secs: 300 }),
    });
    mount();
    const form = await openForm();
    // Opening the form asks once (the preset it opens on).
    await waitFor(() => expect(calls.filter((c) => c.path.endsWith("/preview"))).toHaveLength(1));
    expect(calls.find((c) => c.path.endsWith("/preview"))?.body).toEqual({ cron: "0 9 * * *", timezone: "Europe/Zurich" });
    expect(await within(form).findByText(/next: /)).toBeInTheDocument();
    // A click on a preset asks at once, with no wait; going back to one already seen asks nothing.
    fireEvent.click(within(form).getByRole("radio", { name: "Weekdays" }));
    await waitFor(() => expect(calls.filter((c) => c.path.endsWith("/preview"))).toHaveLength(2));
    fireEvent.click(within(form).getByRole("radio", { name: "Every day" }));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
    const bodies = calls.filter((c) => c.path.endsWith("/preview")).map((c) => (c.body as { cron: string }).cron);
    expect(bodies).toEqual(["0 9 * * *", "0 9 * * 1-5"]);
  });
});

describe("a webhook row", () => {
  it("shows the address and secret once on create, with How to call it, and never keeps them in the cache", async () => {
    const url = "https://aurora.example/hooks/whk_SECRETTOKEN";
    const calls = mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([hook()]),
      "POST /v1/agents/a1/webhooks": () => json(201, { url, secret: "GENERATEDSECRET1234567890abcdef12", webhook: hook() }),
    });
    mount();
    await screen.findByText("Deploy notice");
    fireEvent.click(screen.getAllByRole("button", { name: "New automation" })[0]!);
    fireEvent.click(await screen.findByRole("button", { name: /When something calls it/ }));
    const form = await screen.findByRole("form", { name: "New webhook" });
    // Signed is the default and says who may call it.
    expect(within(form).getByRole("radio", { name: /Signed/ })).toBeChecked();
    expect(within(form).getByRole("radio", { name: /Bearer token/ })).toBeInTheDocument();
    expect(within(form).getByRole("radio", { name: /Anyone with the URL/ })).toBeInTheDocument();
    expect(within(form).getByText("Only senders that know the secret can wake Daneel.")).toBeInTheDocument();
    // (webhooks-2) The agent's own name, never a sample one.
    expect(within(form).getByText("Tells Daneel what to do with each delivery.")).toBeInTheDocument();
    expect(form.textContent).not.toContain("Dani");
    expect(form.textContent).not.toContain("hmac_sha256");
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "Deploy notice" } });
    fireEvent.change(within(form).getByLabelText("Instructions"), { target: { value: "Handle it" } });
    fireEvent.click(within(form).getByRole("button", { name: "Create webhook" }));
    const dialog = await screen.findByRole("dialog");
    // Empty secret on Signed: the server makes one.
    expect(calls.find((c) => c.method === "POST" && c.path === "/v1/agents/a1/webhooks")?.body).toMatchObject({ verification: { kind: "hmac_sha256", generate_secret: true } });
    const boxes = within(dialog).getAllByTestId("secret-key");
    expect(boxes.map((b) => b.textContent)).toEqual([url, "GENERATEDSECRET1234567890abcdef12"]);
    expect(within(dialog).getByRole("heading", { name: "How to call it" })).toBeInTheDocument();
    const snippet = within(dialog).getByTestId("howto-snippet").textContent ?? "";
    expect(snippet).toContain(url);
    expect(snippet).toContain("X-Signature: sha256=");
    expect(snippet).not.toContain("GENERATEDSECRET");
    expect(JSON.stringify(qcRef.getQueryCache().getAll().map((q) => q.state.data))).not.toContain("SECRETTOKEN");
    expect(JSON.stringify(qcRef.getQueryCache().getAll().map((q) => q.state.data))).not.toContain("GENERATEDSECRET");
    fireEvent.click(within(dialog).getByRole("button", { name: "I have saved it" }));
    fireEvent.click(await screen.findByRole("button", { name: "Close anyway" }));
    await waitFor(() => expect(screen.queryByTestId("secret-key")).toBeNull());
    expect(document.body.textContent).not.toContain("SECRETTOKEN");
  });

  it("Generate fills a random 32-character secret and it is sent with the create", async () => {
    const calls = mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([]), "POST /v1/agents/a1/webhooks": () => json(201, { url: "https://x/whk_1", webhook: hook() }) });
    mount();
    fireEvent.click((await screen.findAllByRole("button", { name: "New automation" }))[0]!);
    fireEvent.click(await screen.findByRole("button", { name: /When something calls it/ }));
    const form = await screen.findByRole("form", { name: "New webhook" });
    fireEvent.click(within(form).getByRole("button", { name: "Generate" }));
    const input = within(form).getByLabelText("Signing secret") as HTMLInputElement;
    expect(input.value).toMatch(/^[A-Za-z0-9]{32}$/);
    const generated = input.value;
    fireEvent.click(within(form).getByRole("button", { name: "Generate" }));
    expect((within(form).getByLabelText("Signing secret") as HTMLInputElement).value).not.toBe(generated);
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "n" } });
    fireEvent.change(within(form).getByLabelText("Instructions"), { target: { value: "i" } });
    fireEvent.click(within(form).getByRole("button", { name: "Create webhook" }));
    await waitFor(() => expect((calls.find((c) => c.method === "POST" && c.path === "/v1/agents/a1/webhooks")?.body as { verification: { secret: string } }).verification.secret).toMatch(/^[A-Za-z0-9]{32}$/));
  });

  it("How to call it on the card uses the address prefix and says the full address was shown once", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([hook()]) });
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "How to call it" }));
    const panel = await screen.findByRole("region", { name: "How to call it" });
    expect(within(panel).getByTestId("howto-snippet").textContent).toContain("https://aurora.example/hooks/whk_ab12…");
    expect(within(panel).getByText("The full address was shown once. Rotate it to get a new one.")).toBeInTheDocument();
    fireEvent.click(within(panel).getByRole("tab", { name: "Python" }));
    expect(within(panel).getByTestId("howto-snippet").textContent).toContain("urllib.request");
  });

  it("Copy puts the snippet on the clipboard", async () => {
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([hook()]) });
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "How to call it" }));
    const panel = await screen.findByRole("region", { name: "How to call it" });
    fireEvent.click(within(panel).getByRole("button", { name: "Copy the curl example" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect((writeText.mock.calls[0] as unknown as string[])[0]).toContain("openssl dgst -sha256");
  });

  it("Rotate URL asks with the consequence, Cancel changes nothing, Rotate shows the new URL once", async () => {
    const calls = mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([hook()]),
      "POST /v1/webhooks/w1/rotate": () => json(200, { url: "https://x/whk_NEW", webhook: hook() }),
    });
    mount();
    await screen.findByText("Deploy notice");
    await openMenu("Deploy notice", "Rotate URL…");
    let dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("The old URL stops working now. Update every sender that uses it. The new URL is shown once.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(calls.some((c) => c.path.endsWith("/rotate"))).toBe(false);
    await openMenu("Deploy notice", "Rotate URL…");
    dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Rotate URL" }));
    expect((await screen.findByTestId("secret-key")).textContent).toContain("whk_NEW");
  });

  it("New secret asks, then shows the new secret once", async () => {
    mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([hook()]),
      "POST /v1/webhooks/w1/rotate-secret": () => json(200, { webhook: hook(), secret: "NEWSECRETVALUE00000000000000000001" }),
    });
    mount();
    await screen.findByText("Deploy notice");
    await openMenu("Deploy notice", "New secret…");
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Make a new secret?");
    expect(dialog).toHaveTextContent("Signatures made with the old secret are refused from now on. Update the sender with the new one first, or deliveries will fail.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Make new secret" }));
    expect((await screen.findByTestId("secret-key")).textContent).toBe("NEWSECRETVALUE00000000000000000001");
  });

  it("Delete names the webhook and what is kept", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([hook()]) });
    mount();
    await screen.findByText("Deploy notice");
    await openMenu("Deploy notice", "Delete…");
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Delete Deploy notice?");
    expect(dialog).toHaveTextContent("The webhook and its delivery history are removed. Senders that call it get an error. Conversations it started are kept.");
    expect(within(dialog).getByRole("button", { name: "Delete webhook" })).toBeInTheDocument();
  });

  it("Deliveries is a table with results in words and a link to each delivery's thread", async () => {
    const d = (id: string, over: Record<string, unknown>) => ({ id, received_at: "2026-01-02T09:00:00Z", size: 31, status: "accepted", result: "answered", conversation_id: null, duplicate_of: null, ...over });
    mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([hook()]),
      "GET /v1/webhooks/w1/deliveries": page([
        d("d1", { conversation_id: "conv-1" }),
        d("d2", { status: "rejected_signature", result: "refused_signature" }),
        d("d3", { status: "duplicate", result: "duplicate" }),
      ]),
    });
    mount();
    await screen.findByText("Deploy notice");
    await openMenu("Deploy notice", "Deliveries");
    const table = await screen.findByRole("table", { name: "Deliveries" });
    expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["When", "Result", "Size", "Conversation"]);
    expect(within(table).getByText("Accepted")).toBeInTheDocument();
    expect(within(table).getByText("Refused: wrong signature")).toHaveClass("text-danger-ink");
    expect(within(table).getByText("Duplicate, ignored")).not.toHaveClass("text-danger-ink");
    expect(within(table).getByRole("link", { name: "Open the conversation" })).toHaveAttribute("href", "/o/acme/w/main/agents/daneel?tab=chat&c=conv-1");
    expect(table.textContent).not.toContain("rejected_signature");
    expect(within(table).getAllByText("None")).toHaveLength(2);
  });

  it("Pause is immediate and Undo resumes", async () => {
    const calls = mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([hook()]), "PATCH /v1/webhooks/w1": () => json(200, hook({ enabled: false })) });
    mount();
    await screen.findByText("Deploy notice");
    await openMenu("Deploy notice", "Pause");
    await waitFor(() => expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ enabled: false }));
    expect(await screen.findByText("Paused Deploy notice")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "PATCH")).toHaveLength(2));
  });
});

describe("Chats", () => {
  it("a chat the agent is in is a row: platform, On, Open channel, and Remove from channel… says what changes", async () => {
    const presence = { id: "p1", agent_id: "a1", connector_id: "mattermost", integration_id: "i1", mode: "shared", status: "active", tenant_label: "chat.example.com", created_at: "2026-01-01T00:00:00Z" };
    const calls = mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([]),
      "GET /v1/agents/a1/presences": page([presence]),
      "GET /v1/agents/a1/presence-options": page([{ integration_id: "i1", connector_id: "mattermost", connector_name: "Mattermost", tenant_label: "chat.example.com", modes: ["shared"], presence_setup: {}, attached: true }]),
      "DELETE /v1/presences/p1": () => json(204, null),
    });
    mount();
    const list = await screen.findByRole("list", { name: "Automations" });
    expect(within(list).getByText("Mattermost · chat.example.com")).toBeInTheDocument();
    expect(within(list).getByText("In chats on Mattermost")).toBeInTheDocument();
    expect(within(list).getByText("On")).toBeInTheDocument();
    expect(within(list).getByRole("button", { name: "Open channel" })).toBeInTheDocument();
    await openMenu("Mattermost · chat.example.com", "Remove from channel…");
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Daneel stops answering in chat.example.com");
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove from channel" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/presences/p1")).toBe(true));
  });

  it("a member on the empty page can ask for a chat platform, with a reason (channels-3)", async () => {
    const calls = mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([]), "POST /v1/workspaces/w1/requests": () => json(200, { id: "r1" }) });
    mount({ canOwn: false });
    expect(await screen.findByRole("heading", { name: "No automations yet" })).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Daneel isn't in any chat yet" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText(/Why do you need it\?/), { target: { value: "For #ops" } });
    fireEvent.click(screen.getByRole("button", { name: "Send request" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST")?.body).toEqual({ subject: "chat_platform", reason: "For #ops" }));
  });

  it("a personal-workspace owner who is only an org member sees the chat-platform ask on the empty page (channels-3)", async () => {
    mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([]),
      "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", my_role: "member" }),
    });
    mount({ canOwn: true });
    expect(await screen.findByRole("heading", { name: "Daneel isn't in any chat yet" })).toBeInTheDocument();
  });

  it("an org admin with no platform does not get the ask on the empty page", async () => {
    mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([]),
      "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", my_role: "admin" }),
    });
    mount({ canOwn: true });
    expect(await screen.findByRole("heading", { name: "No automations yet" })).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("heading", { name: "Daneel isn't in any chat yet" })).not.toBeInTheDocument());
  });

  it("with no chat platform the Chats filter shows Ask an admin with the platforms and a reason", async () => {
    const calls = mockApi({ "GET /v1/agents/a1/schedules": page([]), "GET /v1/agents/a1/webhooks": page([]), "POST /v1/workspaces/w1/requests": () => json(200, { id: "r1" }) });
    mount({ initialFilter: "chats" });
    expect(await screen.findByRole("heading", { name: "Daneel isn't in any chat yet" })).toBeInTheDocument();
    expect(screen.getByText("Only organization owners and admins can connect a chat platform.")).toBeInTheDocument();
    expect(await screen.findByText("to connect Mattermost or Slack", { exact: false })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText(/Why do you need it\?/), { target: { value: "So Daneel can answer in #ops" } });
    fireEvent.click(screen.getByRole("button", { name: "Send request" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST")?.body).toEqual({ subject: "chat_platform", reason: "So Daneel can answer in #ops" }));
    expect(await screen.findByText("Asked an admin")).toBeInTheDocument();
  });
});

// (P5-SCHED-ONCE, Part C, D-407, D-421) Every year and Once in the schedule form; a one-time schedule's row: pending, Waiting, Done.
describe("one-time and yearly schedules", () => {
  const form422 = (message: string) => json(422, { error: { code: "invalid_run_at", message, details: { fields: [{ path: "/run_at", message }] } } });
  async function openForm() {
    fireEvent.click((await screen.findAllByRole("button", { name: "New automation" }))[0]!);
    fireEvent.click(await screen.findByRole("button", { name: /On a schedule/ }));
    return screen.findByRole("form", { name: "New schedule" });
  }
  const once = (over: Record<string, unknown> = {}) => sched({ cron: null, run_at: "2026-11-21T09:00:00", next_fire_at: "2026-11-21T08:00:00Z", next_runs: ["2026-11-21T08:00:00Z"], name: "Water the plants", ...over });

  it("Every year: day, month and time make the cron; the summary names the next run with its year", async () => {
    const calls = mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([]),
      "POST /v1/agents/a1/schedules/preview": () => json(200, { next: ["2027-11-27T09:00:00Z"], interval_secs: null, too_frequent: false, min_interval_secs: 300 }),
      "POST /v1/agents/a1/schedules": () => json(201, sched()),
    });
    mount();
    const form = await openForm();
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "Anniversary" } });
    fireEvent.change(within(form).getByLabelText("Prompt"), { target: { value: "remind me" } });
    fireEvent.click(within(form).getByRole("radio", { name: "Every year" }));
    fireEvent.change(within(form).getByRole("combobox", { name: "Day of the month" }), { target: { value: "27" } });
    fireEvent.change(within(form).getByRole("combobox", { name: "Month" }), { target: { value: "11" } });
    fireEvent.change(within(form).getByRole("combobox", { name: "Time of day" }), { target: { value: "10:00" } });
    await waitFor(() => expect(within(form).getByTestId("schedule-summary")).toHaveTextContent("Every year on 27 November at 10:00 · next: 27 Nov 2027 (Zurich time)"), { timeout: 3000 });
    fireEvent.click(within(form).getByRole("button", { name: "Create schedule" }));
    await waitFor(() => expect(created(calls)?.body).toMatchObject({ name: "Anniversary", cron: "0 10 27 11 *", timezone: "Europe/Zurich" }));
    expect(created(calls)?.body).not.toHaveProperty("run_at");
  });

  it("Once: a date (tomorrow) and a time (09:00) in the zone make run_at and no cron", async () => {
    const calls = mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([]),
      "POST /v1/agents/a1/schedules/preview": () => json(200, { next: ["2026-11-21T08:00:00Z"], interval_secs: null, too_frequent: false, min_interval_secs: 300 }),
      "POST /v1/agents/a1/schedules": () => json(201, once()),
    });
    mount();
    const form = await openForm();
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "Water the plants" } });
    fireEvent.change(within(form).getByLabelText("Prompt"), { target: { value: "remind me" } });
    fireEvent.click(within(form).getByRole("radio", { name: "Once" }));
    const date = within(form).getByLabelText("Date") as HTMLInputElement;
    expect(date.value).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    expect(within(form).getByRole("combobox", { name: "Time of day" })).toHaveValue("09:00");
    expect(within(form).getByRole("combobox", { name: "Time zone" })).toHaveValue("Europe/Zurich · Acme workspace");
    fireEvent.change(date, { target: { value: "2026-11-21" } });
    expect(within(form).getByTestId("schedule-summary")).toHaveTextContent("Once, on Sat 21 Nov at 09:00 (Zurich time).");
    expect(within(form).getByTestId("schedule-summary")).not.toHaveTextContent("at most every 5 minutes");
    fireEvent.click(within(form).getByRole("button", { name: "Create schedule" }));
    await waitFor(() => expect(created(calls)?.body).toMatchObject({ name: "Water the plants", run_at: "2026-11-21T09:00", timezone: "Europe/Zurich" }));
    expect(created(calls)?.body).not.toHaveProperty("cron");
  });

  it("Once: a time in the past says 'Pick a time in the future' as you type and is not sent", async () => {
    const calls = mockApi({
      "GET /v1/agents/a1/schedules": page([]),
      "GET /v1/agents/a1/webhooks": page([]),
      "POST /v1/agents/a1/schedules/preview": (req) => req.clone().text().then((t) => (JSON.parse(t).run_at ? form422("is in the past") : json(200, { next: [], interval_secs: null, too_frequent: false, min_interval_secs: 300 }))),
      "POST /v1/agents/a1/schedules": () => json(201, once()),
    });
    mount();
    const form = await openForm();
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "Late" } });
    fireEvent.change(within(form).getByLabelText("Prompt"), { target: { value: "go" } });
    fireEvent.click(within(form).getByRole("radio", { name: "Once" }));
    fireEvent.change(within(form).getByLabelText("Date"), { target: { value: "2020-01-01" } });
    expect(await within(form).findByText("Pick a time in the future", undefined, { timeout: 3000 })).toBeInTheDocument();
    expect(within(form).getByRole("button", { name: "Create schedule" })).toBeDisabled();
    fireEvent.submit(form);
    expect(created(calls)).toBeUndefined();
  });

  it("a pending one-time schedule reads 'Once · Sat 21 Nov at 09:00 (Zurich time)' with On / Paused; Edit opens on Once", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([once(), once({ id: "s2", name: "Paused one", enabled: false, disabled_reason: "user" })]), "GET /v1/agents/a1/webhooks": page([]) });
    mount();
    await screen.findByText("Water the plants");
    expect(screen.getAllByText("Once · Sat 21 Nov at 09:00 (Zurich time)")).toHaveLength(2);
    expect(screen.getByText("On")).toBeInTheDocument();
    expect(screen.getByText("Paused")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]!);
    const form = await screen.findByRole("form", { name: "Edit schedule" });
    expect(within(form).getByRole("radio", { name: "Once" })).toBeChecked();
    expect(within(form).getByLabelText("Date")).toHaveValue("2026-11-21");
  });

  it("Waiting: a due one-time schedule of a paused agent says it runs when the agent is resumed", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([once({ next_fire_at: "2020-01-01T08:00:00Z" })]), "GET /v1/agents/a1/webhooks": page([]) });
    mount({ agentFrozen: true });
    expect(await screen.findByText("Waiting: Daneel is paused. It runs when Daneel is resumed.")).toBeInTheDocument();
  });

  it("no Waiting while the agent runs, or before the time has come", async () => {
    mockApi({ "GET /v1/agents/a1/schedules": page([once({ next_fire_at: "2020-01-01T08:00:00Z" }), once({ id: "s2", name: "Later", next_fire_at: "2999-01-01T08:00:00Z" })]), "GET /v1/agents/a1/webhooks": page([]) });
    mount();
    await screen.findByText("Later");
    expect(screen.queryByText(/Waiting:/)).toBeNull();
  });

  it("a schedule row's next run is a moment in the person's zone, with no zone words; the cadence keeps the schedule's own (b2#18)", async () => {
    mockApi({
      "GET /v1/agents/a1/schedules": page([sched({ next_runs: ["2026-11-22T08:00:00Z"], next_fire_at: "2026-11-22T08:00:00Z" })]),
      "GET /v1/agents/a1/webhooks": page([]),
    });
    setDisplayTimeZone("Asia/Tokyo");
    try {
      mount();
      await screen.findByText("Morning summary");
      // 08:00 UTC is 17:00 in Tokyo; the cadence line still says Zurich, the schedule's setting.
      expect((await screen.findAllByText(/next at 17:00$/)).length).toBeGreaterThan(0);
      expect(screen.queryByText(/next at .*\(.* time\)/)).toBeNull();
      expect(screen.getAllByText(/Every day at 09:00 · Zurich · next at 17:00/).length).toBeGreaterThan(0);
    } finally {
      setDisplayTimeZone(null);
    }
  });

  it("Done: badge Done, 'Ran on … · removed on …', Run again and Delete, no Pause or Edit; Run again opens Once on a new date", async () => {
    const calls = mockApi({
      "GET /v1/agents/a1/schedules": page([once({ completed_at: "2026-11-21T08:00:00Z", next_fire_at: null, next_runs: [] }), sched({ id: "s3", name: "Recurring" })]),
      "GET /v1/agents/a1/webhooks": page([]),
      "PATCH /v1/schedules/s1": () => json(200, once()),
    });
    // What it did is a moment, in the person's zone (b2#18); the schedule's own zone (Zurich too here) is only its setting.
    setDisplayTimeZone("Europe/Zurich");
    mount();
    await screen.findByText("Water the plants");
    expect(screen.getByText("Done")).toBeInTheDocument();
    expect(screen.getByText("Ran on Sat 21 Nov at 09:00 · removed on 21 Dec")).toBeInTheDocument();
    setDisplayTimeZone(null);
    // Done counts as neither On nor Paused: the recurring one is the only On.
    expect(screen.getAllByText("On")).toHaveLength(1);
    expect(screen.queryByText("Paused")).toBeNull();
    expect(screen.getAllByRole("button", { name: "Edit" })).toHaveLength(1);
    for (const b of screen.getAllByRole("button", { name: "Run now" })) expect(b).toBeEnabled();
    await openMenu("Water the plants", "Delete…");
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.keyDown(screen.getByRole("button", { name: "More for Water the plants" }), { key: "Enter" });
    expect(screen.queryByRole("menuitem", { name: "Pause" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Resume" })).toBeNull();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: "Run again" }));
    const form = await screen.findByRole("form", { name: "Edit schedule" });
    expect(within(form).getByRole("radio", { name: "Once" })).toBeChecked();
    expect(within(form).getByLabelText("Date")).not.toHaveValue("2026-11-21");
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.find((c) => c.method === "PATCH")?.body).toMatchObject({ run_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T09:00$/), timezone: "Europe/Zurich" }));
    expect(calls.find((c) => c.method === "PATCH")?.body).not.toHaveProperty("cron");
  });
});
