import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/internet-access",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { GovernanceFields } from "@/components/admin/settings/governance-settings-view";
import { SettingsSectionFields } from "@/components/admin/settings/settings-section-fields";
import { SettingsIndexView, groupSections } from "@/components/admin/settings/settings-index-view";
import { adminRoutes } from "@/lib/routes";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = typeof input === "string" ? new Request(new URL(input, "http://localhost")) : (input as Request);
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const schema = (kind: string, extra: Record<string, unknown> = {}) => ({
  kind,
  min: null,
  max: null,
  max_len: null,
  options: null,
  item_kind: null,
  ...extra,
});
const setting = (key: string, over: Record<string, unknown> = {}) => ({
  key,
  value: null,
  source: "default",
  locked: false,
  helm_key: `config.${key}`,
  default: null,
  schema: schema("string"),
  list: null,
  id_field: null,
  entries: null,
  secret: null,
  ...over,
});
const bool = (key: string, value: boolean, source = "default") =>
  setting(key, { value, default: value, source, schema: schema("bool") });

/** A governance document with one dashboard value in each of the three topics' keys. */
function governance() {
  return {
    section: "governance",
    title: "Governance",
    description: "The instance ceiling.",
    generation: 5,
    updated_at: null,
    updated_by_label: null,
    extra: {},
    settings: [
      bool("governance.llm.require_key_origin", true, "dashboard"),
      setting("governance.egress.baseline_allowlist", { value: [], list: "whole", schema: schema("list", { item_kind: "string" }) }),
      setting("governance.egress.minimum_posture", { value: "open", default: "open", schema: schema("enum", { options: ["open", "credentialed_only"] }) }),
      setting("governance.egress.open_lane_minimum", { value: "enabled", default: "enabled", schema: schema("enum", { options: ["enabled", "disabled"] }) }),
      bool("byo.enabled", true, "dashboard"),
      setting("governance.agent_tokens.max_age_days", { value: 30, source: "dashboard", schema: schema("int", { min: 1, max: 3650 }) }),
    ],
  };
}

describe("SettingsSectionFields (C-971)", () => {
  it("shows only the chosen keys but saves the whole document, untouched keys unchanged", async () => {
    let put: { generation: number; values: Record<string, unknown> } | null = null;
    mockFetch({
      "GET /v1/admin/settings/governance": () => json(200, governance()),
      "POST /v1/admin/settings/governance/preview": () =>
        json(200, { generation: 5, diff: [], loosening: [], affected_orgs: [], affected_orgs_total: 0, warnings: [] }),
      "PUT /v1/admin/settings/governance": async (req) => {
        put = (await req.json()) as typeof put;
        return json(200, governance());
      },
    });
    wrap(<GovernanceFields part="internet" />);
    await screen.findByText("Egress", { selector: "legend" });
    // Only the egress keys are on this page.
    expect(document.getElementById("governance.egress.minimum_posture")).toBeTruthy();
    expect(document.getElementById("byo.enabled")).toBeNull();
    expect(document.getElementById("governance.llm.require_key_origin")).toBeNull();
    fireEvent.change(document.getElementById("governance.egress.minimum_posture-c")!, { target: { value: "credentialed_only" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(put).not.toBeNull());
    const sent = put as unknown as { generation: number; values: Record<string, unknown> };
    expect(sent.generation).toBe(5);
    expect(sent.values["governance.egress.minimum_posture"]).toBe("credentialed_only");
    // The other topics' dashboard values travel with it, unchanged.
    expect(sent.values["byo.enabled"]).toBe(true);
    expect(sent.values["governance.llm.require_key_origin"]).toBe(true);
    expect(sent.values["governance.agent_tokens.max_age_days"]).toBe(30);
  });

  it("takes a plain `keys` subset with its own title", async () => {
    mockFetch({ "GET /v1/admin/settings/governance": () => json(200, governance()) });
    wrap(<SettingsSectionFields section="governance" keys={["byo.enabled"]} title="BYO switch" />);
    await screen.findByText("BYO switch", { selector: "legend" });
    expect(document.getElementById("byo.enabled")).toBeTruthy();
    expect(document.getElementById("governance.egress.minimum_posture")).toBeNull();
  });
});

describe("All settings (C-971)", () => {
  const sections = [
    { section: "governance", title: "Governance", description: "Floors for every organization.", generation: 0, updated_at: null, updated_by_label: null, keys_total: 4, keys_locked: 0, keys_dashboard: 0 },
    { section: "sign_in", title: "Sign-in", description: "Providers and sessions.", generation: 0, updated_at: null, updated_by_label: null, keys_total: 3, keys_locked: 0, keys_dashboard: 0 },
    { section: "retention", title: "Retention", description: "How long Aurora keeps things.", generation: 0, updated_at: null, updated_by_label: null, keys_total: 3, keys_locked: 0, keys_dashboard: 0 },
  ] as Parameters<typeof groupSections>[0];

  it("groups sections by topic in sidebar order and filters by name or description", () => {
    expect(groupSections(sections, "").map((g) => g.topic)).toEqual(["AI models", "People", "Data"]);
    expect(groupSections(sections, "providers").map((g) => g.sections.map((s) => s.section))).toEqual([["sign_in"]]);
    expect(groupSections(sections, "KEEPS").map((g) => g.topic)).toEqual(["Data"]);
    expect(groupSections(sections, "zzz")).toEqual([]);
  });

  it("links each section to its topic tab, governance to all three of its homes, and searches", async () => {
    mockFetch({ "GET /v1/admin/settings": () => json(200, { sections, locked: [] }) });
    wrap(<SettingsIndexView />);
    await screen.findByRole("heading", { name: "All settings" });
    expect(screen.getByRole("link", { name: "Open Sign-in" }).getAttribute("href")).toBe(adminRoutes.signIn());
    expect(screen.getByRole("link", { name: "Open Governance" }).getAttribute("href")).toBe("/admin/ai-models?tab=rules");
    expect(screen.getByRole("link", { name: "Internet access" }).getAttribute("href")).toBe("/admin/internet-access?tab=limits");
    expect(screen.getByRole("link", { name: "Agents › BYO" }).getAttribute("href")).toBe("/admin/agents?tab=byo");
    fireEvent.change(screen.getByRole("searchbox", { name: "Search settings" }), { target: { value: "retention" } });
    expect(screen.queryByText("Sign-in")).toBeNull();
    expect(screen.getByText("Retention", { selector: "a" })).toBeTruthy();
    fireEvent.change(screen.getByRole("searchbox", { name: "Search settings" }), { target: { value: "nothing like this" } });
    expect(screen.getByRole("status").textContent).toMatch(/No settings match/);
  });
});
