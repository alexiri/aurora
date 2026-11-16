import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/agents",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { SettingsIndexView, groupSections, matchingFields } from "@/components/admin/settings/settings-index-view";
import { SettingsSectionFields } from "@/components/admin/settings/settings-section-fields";
import { SettingsSectionForm } from "@/components/admin/settings/settings-section-form";
import type { SettingsSection } from "@/lib/api/settings";
import { durationWords, valueWords } from "@/lib/settings-words";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
function mockFetch(routes: Record<string, () => Response>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url, "http://localhost");
      const h = routes[`${input.method} ${url.pathname}`];
      return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}
const wrap = (ui: React.ReactNode) =>
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const schema = (kind: string, extra: Record<string, unknown> = {}) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null, ...extra });
const setting = (over: Record<string, unknown>) => ({
  key: "x", value: null, source: "default", locked: false, helm_key: "config.x", default: null, schema: schema("string"),
  list: null, id_field: null, entries: null, secret: null, ...over,
});
const fld = (key: string, label: string, kind: string, over: Record<string, unknown> = {}) => ({
  key, label, help: `${label}: what it does.`, kind, choices: null, default_display: "None", unit: null, doc: null, ...over,
});

function agents(over: Record<string, unknown> = {}): SettingsSection {
  return {
    section: "agents",
    title: "Agents",
    description: "Defaults.",
    generation: 1,
    updated_at: null,
    updated_by_label: null,
    extra: null,
    editable: true,
    current: null,
    fields: [
      fld("approvals.timeout_secs", "How long a question waits", "duration", { unit: "minutes", default_display: "15 minutes" }),
      fld("byo.minimum_posture", "Who may connect their own agent", "enum", {
        default_display: "Workspace owners only",
        choices: [
          { value: "enabled", label: "Anyone in a workspace", description: "Any member may connect." },
          { value: "owners_only", label: "Workspace owners only", description: "Only owners may connect." },
          { value: "disabled", label: "Nobody", description: "Nobody may connect." },
        ],
      }),
      fld("scim.allowed_email_domains", "E-mail domains", "list", { default_display: "Any" }),
    ],
    settings: [
      setting({ key: "approvals.timeout_secs", value: 900, default: 900, schema: schema("int", { min: 0, max: 2592000 }) }),
      setting({ key: "byo.minimum_posture", value: "owners_only", default: "owners_only", schema: schema("enum", { options: ["enabled", "owners_only", "disabled"] }) }),
      setting({ key: "scim.allowed_email_domains", value: ["a.example"], default: ["a.example"], list: "whole", schema: schema("list", { item_kind: "string" }) }),
    ],
    ...over,
  } as unknown as SettingsSection;
}

describe("Settings in words (C-972, D-341)", () => {
  it("renders labels, help, choice cards, a duration in its unit and a list editor — no key, no raw value", async () => {
    mockFetch({ "GET /v1/admin/settings/agents": () => json(200, agents()) });
    wrap(<SettingsSectionFields section="agents" />);
    expect(await screen.findByText("How long a question waits")).toBeTruthy();
    expect(screen.getByText("How long a question waits: what it does.")).toBeTruthy();
    // 900 seconds is edited as 15 minutes, with the Default badge saying what the default is.
    expect((document.getElementById("approvals.timeout_secs-c") as HTMLInputElement).value).toBe("15");
    expect(screen.getByText("minutes")).toBeTruthy();
    expect(screen.getByTestId("approvals.timeout_secs-default").textContent).toBe("Default: 15 minutes");
    // The enumeration is choice cards with a sentence each, never the stored id.
    const group = screen.getByRole("radiogroup", { name: "Who may connect their own agent" });
    expect(within(group).getAllByRole("radio")).toHaveLength(3);
    expect(within(group).getByText("Only owners may connect.")).toBeTruthy();
    expect((within(group).getByRole("radio", { name: /Workspace owners only/ }) as HTMLInputElement).checked).toBe(true);
    // The list is one input per entry with Add, never JSON or a text blob.
    expect((screen.getByLabelText("E-mail domains: entry 1") as HTMLInputElement).value).toBe("a.example");
    expect(screen.getByRole("button", { name: "Add" })).toBeTruthy();
    const text = document.body.textContent ?? "";
    for (const raw of ["owners_only", "approvals.timeout_secs", "byo.minimum_posture", "[]", "labelOf"]) {
      expect(text, raw).not.toContain(raw);
    }
  });

  it("saves a duration back in seconds and a chosen option by its stored id", async () => {
    let put: { values: Record<string, unknown> } | null = null;
    mockFetch({
      "GET /v1/admin/settings/agents": () => json(200, agents()),
      "POST /v1/admin/settings/agents/preview": () => json(200, { generation: 1, diff: [], loosening: [], affected_orgs: [], affected_orgs_total: 0, warnings: [] }),
      "PUT /v1/admin/settings/agents": () => json(200, agents()),
    });
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    wrap(<SettingsSectionFields section="agents" />);
    await screen.findByText("How long a question waits");
    fireEvent.change(document.getElementById("approvals.timeout_secs-c")!, { target: { value: "20" } });
    fireEvent.click(screen.getByRole("radio", { name: /Nobody/ }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(fetchSpy.mock.calls.some(([r]) => (r as Request).method === "PUT")).toBe(true));
    const req = fetchSpy.mock.calls.map(([r]) => r as Request).find((r) => r.method === "PUT")!;
    put = (await req.clone().json()) as typeof put;
    expect(put?.values["approvals.timeout_secs"]).toBe(1200);
    expect(put?.values["byo.minimum_posture"]).toBe("disabled");
  });

  it("shows a section that is not editable read-only: current values in words, a source chip, no Save", () => {
    const readOnly = agents({
      editable: false,
      current: [
        { key: "approvals.timeout_secs", label: "How long a question waits", value_display: "15 minutes", source: "default" },
        { key: "email.smtp_password", label: "Password", value_display: "Set", source: "helm" },
      ],
      fields: [fld("approvals.timeout_secs", "How long a question waits", "duration", { doc: { title: "Rate limits, approvals and e-mail", path: "docs/operator/limits-and-approvals.md" } })],
    });
    wrap(<SettingsSectionForm section={readOnly} onReload={() => undefined} />);
    expect(screen.getByTestId("approvals.timeout_secs-current").textContent).toBe("15 minutes");
    expect(screen.getByTestId("email.smtp_password-current").textContent).toBe("Set");
    expect(screen.getByText("Helm")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(document.querySelector("form")).toBeNull();
    expect(document.body.textContent).toContain("Change them in your Helm values");
    expect(document.body.textContent).toContain("Rate limits, approvals and e-mail");
    expect(document.body.textContent).not.toContain("docs/");
  });

  it("words: durations in the largest whole unit, options by their label, lists as entries", () => {
    expect(durationWords(900)).toBe("15 minutes");
    expect(durationWords(3600)).toBe("1 hour");
    expect(durationWords(45)).toBe("45 seconds");
    const f = agents().fields![1]!;
    expect(valueWords(f, "owners_only")).toBe("Workspace owners only");
    expect(valueWords(undefined, [])).toBe("None");
    expect(valueWords(undefined, true)).toBe("On");
  });
});

describe("All settings search (C-972)", () => {
  const row = (over: Record<string, unknown>) => ({
    section: "agents", title: "Agents", description: "Defaults.", generation: 0, updated_at: null, updated_by_label: null,
    keys_total: 1, keys_locked: 0, keys_dashboard: 0, fields: [fld("agents.default_name", "Name of a new person's agent", "text")], ...over,
  });
  const sections = [
    row({ fields: [{ ...fld("agents.default_name", "Name of a new person's agent", "text"), help: "Used until the person picks another." }] }),
    row({ section: "retention", title: "Retention", description: "How long.", fields: [fld("retention.request_log_days", "Keep the activity of requests for", "int")] }),
  ] as unknown as Parameters<typeof groupSections>[0];

  it("finds a field by its label or its help text", () => {
    expect(groupSections(sections, "picks another").flatMap((g) => g.sections.map((s) => s.section))).toEqual(["agents"]);
    expect(groupSections(sections, "activity of requests").flatMap((g) => g.sections.map((s) => s.section))).toEqual(["retention"]);
    expect(matchingFields(sections[0]!, "picks another").map((f) => f.key)).toEqual(["agents.default_name"]);
    expect(matchingFields(sections[0]!, "")).toEqual([]);
  });

  it("lists the matching fields under their section, linking to the setting", async () => {
    mockFetch({ "GET /v1/admin/settings": () => json(200, { sections, locked: [] }) });
    wrap(<SettingsIndexView />);
    await screen.findByRole("heading", { name: "All settings" });
    fireEvent.change(screen.getByRole("searchbox", { name: "Search settings" }), { target: { value: "picks another" } });
    const link = await screen.findByRole("link", { name: "Name of a new person's agent" });
    expect(link.getAttribute("href")).toContain("#agents.default_name");
    expect(screen.queryByText("Retention", { selector: "a" })).toBeNull();
  });
});
