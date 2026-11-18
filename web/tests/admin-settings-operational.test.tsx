import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/settings/agents",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import {
  AGENTS_PAGE,
  APPROVALS_PAGE,
  LIMITS_PAGE,
  PREVIEWS_PAGE,
  RETENTION_PAGE,
  SCIM_PAGE,
  SKILLS_PAGE,
  TERMINAL_PAGE,
} from "@/components/admin/settings/operational-settings-views";
import { SettingsSectionView } from "@/components/admin/settings/settings-section-view";
import { withFields } from "./support/settings-fields";

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- request bodies are inspected loosely
type Json = Record<string, any>;
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = typeof input === "string" ? new Request(new URL(input, "http://localhost")) : (input as Request);
      const url = new URL(req.url, "http://localhost");
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

const schema = (kind: string, o: Json = {}) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null, ...o });
const base = { locked: false, helm_key: "config.x", list: null, id_field: null, entries: null, default: null, secret: null };
const wrap = (slug: string) => (
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <SettingsSectionView slug={slug} />
  </QueryClientProvider>
);
const bodyOf = async (req: Request) => (await req.clone().json()) as Json;

function section(id: string, title: string, settings: Json[], extra: unknown = null): Json {
  return withFields({ section: id, title, description: `${title} description.`, generation: 3, updated_at: null, updated_by_label: null, extra, settings });
}

describe("Admin › Settings › the operational pages (SET-5)", () => {
  it("lays every registry key out once per page", () => {
    // The keys of docs/15 §Instance settings for each section: a page that drops or repeats one fails here.
    const registry: Record<string, string[]> = {
      agents: [
        "agents.default_name", "agents.default_brief", "hosted.enabled", "hosted.default_harness", "hosted.enabled_harnesses",
        "hosted.default_timezone", "hosted.idle_stop_secs", "hosted.background_cap_secs", "hosted.turn_ceiling_secs",
        "hosted.turn_stall_secs", "hosted.sandbox_ready_timeout_secs",
      ],
      approvals: ["approvals.timeout_secs", "approvals.max_timeout_secs", "approvals.email_notifications"],
      retention: [
        "retention.request_log_days", "retention.conversation_days_max", "attachments.default_org_quota_mib",
        "attachments.org_quota_mib_max", "attachments.days_max",
      ],
      skills: ["skills.library.enabled", "skills.library.refresh_interval_secs", "skills.library.bytes_per_agent"],
      scim: ["scim.enabled", "scim.allowed_email_domains", "scim.rate_per_minute", "scim.token_max_days"],
      previews: [
        "previews.session_ttl_secs", "previews.session_idle_secs", "previews.bytes_per_share_day", "previews.shares_per_agent",
        "previews.shares_per_org", "previews.awake_cap_secs",
      ],
      terminal: ["terminal.enabled", "terminal.awake_cap_secs", "terminal.wake_window_secs", "terminal.wake_wait_secs"],
      limits: [
        "ratelimit.api.multiplier", "ratelimit.api.org_per_minute", "ratelimit.api.classes", "gateway.allowed_private",
        "gateway.condition_body_bytes", "gateway.upstream_header_timeout_secs",
      ],
    };
    const pages = { agents: AGENTS_PAGE, approvals: APPROVALS_PAGE, retention: RETENTION_PAGE, skills: SKILLS_PAGE, scim: SCIM_PAGE, previews: PREVIEWS_PAGE, terminal: TERMINAL_PAGE, limits: LIMITS_PAGE };
    for (const [id, page] of Object.entries(pages)) {
      const shown = page.groups.flatMap((g) => g.keys ?? []);
      expect([...shown].sort(), id).toEqual([...registry[id]!].sort());
    }
  });

  it("shows a Helm-locked deadline read-only and the runners' sizes in a read-only table", async () => {
    mockFetch({
      "GET /v1/admin/settings/agents": () =>
        json(
          200,
          section(
            "agents",
            "Agents & sandboxes",
            [
              { ...base, key: "hosted.idle_stop_secs", value: 1800, source: "helm", locked: true, helm_key: "config.hosted.idle_stop_secs", schema: schema("int") },
              { ...base, key: "hosted.turn_ceiling_secs", value: 21600, source: "default", default: 21600, schema: schema("int") },
              { ...base, key: "hosted.enabled_harnesses", value: ["claude-code"], source: "default", schema: schema("list", { item_kind: "string" }) },
            ],
            {
              runners: [
                { id: "r1", name: "main", sandbox_memory_mib: 4096, sandbox_cpus: 2, sandbox_pids: 512, home_gib: 5, max_sandboxes: 7, harnesses: ["claude-code", "codex"] },
              ],
            },
          ),
        ),
    });
    render(wrap("agents"));
    expect(await screen.findByRole("heading", { name: "Agents & sandboxes", level: 1 })).toBeTruthy();
    expect(screen.getByTestId("hosted.idle_stop_secs-locked-value").textContent).toBe("1800");
    expect(screen.getByText("config.hosted.idle_stop_secs")).toBeTruthy();
    expect(document.getElementById("hosted.idle_stop_secs-c")).toBeNull();
    expect((document.getElementById("hosted.turn_ceiling_secs-c") as HTMLInputElement).value).toBe("21600");
    const table = screen.getByRole("region", { name: "Runners and sandbox sizes" });
    const row = within(table).getByRole("row", { name: /main/ });
    expect(row.textContent).toContain("4096");
    expect(row.textContent).toContain("claude-code, codex");
    expect(screen.getByText("runner.max_sandboxes")).toBeTruthy();
    // The table is read-only: nothing in it can be edited.
    expect(within(table).queryAllByRole("textbox")).toHaveLength(0);
  });

  it("explains a terminal the deployment does not provide, and saves the clocks", async () => {
    let put: Json | null = null;
    const terminal = (available: boolean) =>
      section(
        "terminal",
        "Terminal",
        [
          { ...base, key: "terminal.enabled", value: false, source: "default", default: false, schema: schema("bool") },
          { ...base, key: "terminal.awake_cap_secs", value: 14400, source: "default", default: 14400, schema: schema("int") },
          { ...base, key: "terminal.wake_window_secs", value: 60, source: "default", default: 60, schema: schema("int") },
          { ...base, key: "terminal.wake_wait_secs", value: 45, source: "default", default: 45, schema: schema("int") },
        ],
        { available },
      );
    mockFetch({
      "GET /v1/admin/settings/terminal": () => json(200, terminal(false)),
      "POST /v1/admin/settings/terminal/preview": () => json(200, { generation: 3, diff: [], loosening: [], affected_orgs: [], affected_orgs_total: 0, warnings: [] }),
      "PUT /v1/admin/settings/terminal": async (req) => {
        put = await bodyOf(req);
        return json(200, terminal(false));
      },
    });
    render(wrap("terminal"));
    expect(await screen.findByText(/does not provide the terminal/)).toBeTruthy();
    expect(screen.getByText("terminal.enabled: true")).toBeTruthy();
    fireEvent.change(document.getElementById("terminal.wake_wait_secs-c")!, { target: { value: "90" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(put).not.toBeNull());
    expect(put).toEqual({ generation: 3, values: { "terminal.wake_wait_secs": 90 } });
  });

  it("shows no notice for a directory sync the deployment routes", async () => {
    mockFetch({
      "GET /v1/admin/settings/scim": () =>
        json(
          200,
          section(
            "scim",
            "Directory sync",
            [{ ...base, key: "scim.enabled", value: true, source: "default", default: true, schema: schema("bool") }],
            { available: true },
          ),
        ),
    });
    render(wrap("scim"));
    expect(await screen.findByRole("heading", { name: "Directory sync", level: 1 })).toBeTruthy();
    expect(screen.queryByText(/does not route/)).toBeNull();
    expect((document.getElementById("scim.enabled-c") as HTMLInputElement).checked).toBe(true);
  });

  it("edits the keyed rate classes: a Helm class is locked, an override is added and saved", async () => {
    let put: Json | null = null;
    const limits = () =>
      section("limits", "Limits", [
        { ...base, key: "ratelimit.api.multiplier", value: 1, source: "default", default: 1, schema: schema("float", { min: 0, max: 100 }) },
        {
          ...base,
          key: "ratelimit.api.classes",
          value: [{ class: "write", per_minute: 600, burst: 100 }],
          source: "helm",
          list: "keyed",
          id_field: "class",
          default: [],
          schema: schema("list", { item_kind: "object", max: 16 }),
          entries: [{ id: "write", source: "helm", locked: true, value: { class: "write", per_minute: 600, burst: 100 }, warnings: null }],
        },
        { ...base, key: "gateway.allowed_private", value: [], source: "default", default: [], schema: schema("list", { item_kind: "string" }) },
      ]);
    mockFetch({
      "GET /v1/admin/settings/limits": () => json(200, limits()),
      "POST /v1/admin/settings/limits/preview": () => json(200, { generation: 3, diff: [], loosening: [], affected_orgs: [], affected_orgs_total: 0, warnings: [] }),
      "PUT /v1/admin/settings/limits": async (req) => {
        put = await bodyOf(req);
        return json(200, limits());
      },
    });
    render(wrap("limits"));
    expect(await screen.findByRole("heading", { name: "Limits", level: 1 })).toBeTruthy();
    // The danger notice names what the allowed-private list does.
    expect(screen.getByText(/Private destinations\./)).toBeTruthy();
    const list = screen.getByRole("list", { name: "Rate class overrides" });
    expect(within(list).getByText("write")).toBeTruthy();
    expect(within(list).getByText("Managed by Helm")).toBeTruthy();
    // The Helm class cannot be picked again.
    const pick = screen.getByRole("combobox", { name: "Class to override" }) as HTMLSelectElement;
    expect([...pick.options].map((o) => o.value)).not.toContain("write");
    fireEvent.change(pick, { target: { value: "export" } });
    fireEvent.click(screen.getByRole("button", { name: "Add override" }));
    fireEvent.change(screen.getByRole("combobox", { name: "export: unit" }), { target: { value: "per_hour" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "export: limit" }), { target: { value: "12" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "export: burst" }), { target: { value: "3" } });
    // A decimal multiplier is sent as a number.
    fireEvent.change(document.getElementById("ratelimit.api.multiplier-c")!, { target: { value: "0.5" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(put).not.toBeNull());
    expect(put).toEqual({
      generation: 3,
      values: { "ratelimit.api.classes": [{ class: "export", per_hour: 12, burst: 3 }], "ratelimit.api.multiplier": 0.5 },
    });
  });

  it("asks for confirmation naming the organizations a lowered retention ceiling clamps", async () => {
    let put = false;
    const retention = () =>
      section("retention", "Retention", [
        { ...base, key: "retention.conversation_days_max", value: null, source: "default", default: null, schema: schema("int", { min: 1, max: 100000 }) },
      ]);
    mockFetch({
      "GET /v1/admin/settings/retention": () => json(200, retention()),
      "POST /v1/admin/settings/retention/preview": () =>
        json(200, {
          generation: 3,
          diff: [],
          loosening: [],
          affected_orgs: [{ org_id: "o1", slug: "acme", name: "Acme Labs", changes: [{ key: "conversations_days", before: 365, after: 90 }] }],
          affected_orgs_total: 1,
          warnings: [],
        }),
      "PUT /v1/admin/settings/retention": () => {
        put = true;
        return json(200, retention());
      },
    });
    render(wrap("retention"));
    await screen.findByRole("heading", { name: "Retention", level: 1 });
    fireEvent.change(document.getElementById("retention.conversation_days_max-c")!, { target: { value: "90" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Affects 1 organization");
    expect(dialog.textContent).toContain("Acme Labs — conversations days 365 → 90");
    expect(put).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(put).toBe(true));
  });
});
