import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/settings/governance",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { SettingsSectionView } from "@/components/admin/settings/settings-section-view";
import { Locked } from "@/components/governance/controls";
import { withFields } from "./support/settings-fields";

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
const posture = (key: string, options: string[], value: string, over: Record<string, unknown> = {}) =>
  setting(key, { value, default: options[0], schema: schema("enum", { options }), ...over });

function governance() {
  return withFields({
    section: "governance",
    title: "Governance",
    description: "The instance ceiling.",
    generation: 2,
    updated_at: "2026-11-15T10:00:00Z",
    updated_by_label: "root@x.io",
    extra: { orgs_url: "/v1/admin/settings/governance/orgs" },
    settings: [
      posture("governance.llm.minimum_posture", ["open", "sanctioned_only"], "sanctioned_only", { source: "helm", locked: true }),
      setting("governance.llm.allowed_key_scopes", { value: ["organization"], list: "whole", schema: schema("list", { item_kind: "string" }) }),
      setting("governance.llm.require_key_origin", { value: false, default: false, schema: schema("bool") }),
      setting("governance.llm.known_llm_hosts_extra", { value: [], list: "whole", schema: schema("list", { item_kind: "string" }) }),
      posture("governance.egress.minimum_posture", ["open", "credentialed_only"], "open"),
      setting("governance.egress.baseline_allowlist", { value: [], list: "whole", schema: schema("list", { item_kind: "object" }) }),
      posture("governance.egress.open_lane_minimum", ["enabled", "allowlist_only", "disabled"], "enabled"),
      setting("governance.agent_tokens.max_age_days", { value: null, schema: schema("int", { min: 1, max: 3650 }) }),
      setting("byo.enabled", { value: true, default: true, schema: schema("bool") }),
      posture("byo.minimum_posture", ["enabled", "owners_only", "disabled"], "owners_only"),
      setting("byo.default_source_cidrs", { value: [], list: "whole", schema: schema("list", { item_kind: "string" }) }),
      setting("byo.require_context_credentials", { value: false, default: false, schema: schema("bool") }),
    ],
  });
}

const orgs = {
  items: [
    {
      org_id: "o1",
      slug: "acme",
      name: "Acme",
      frozen: false,
      llm_posture: "sanctioned_only",
      egress_posture: "open",
      open_lane: "enabled",
      byo_posture: "owners_only",
      stricter: ["governance.llm.minimum_posture"],
      clamped: [],
    },
    {
      org_id: "o2",
      slug: "globex",
      name: "Globex",
      frozen: true,
      llm_posture: "sanctioned_only",
      egress_posture: "open",
      open_lane: "enabled",
      byo_posture: "owners_only",
      stricter: [],
      clamped: ["byo.minimum_posture"],
    },
  ],
  next_cursor: null,
};

describe("Admin › Settings › Governance (SET-2)", () => {
  it("renders the four sections, the policy notice, a locked key read-only and where organizations stand", async () => {
    mockFetch({
      "GET /v1/admin/settings/governance": () => json(200, governance()),
      "GET /v1/admin/settings/governance/orgs": () => json(200, orgs),
    });
    wrap(<SettingsSectionView slug="governance" />);
    for (const t of ["AI keys", "Internet access", "BYO agents", "Agent tokens", "Where organizations stand"]) {
      await screen.findByText(t, { selector: "legend, h3" });
    }
    expect(screen.getByText(/Rules are set per organization and workspace/)).toBeTruthy();
    // The Helm key is locked: read-only, with the "Managed by Helm" marker.
    const locked = document.getElementById("governance.llm.minimum_posture") as HTMLSelectElement | null;
    expect(locked?.disabled ?? true).toBe(true);
    expect(screen.getAllByText(/Helm/i).length).toBeGreaterThan(0);
    // The table.
    const table = await screen.findByRole("region", { name: "Organizations and their effective governance" });
    expect(within(table).getByText("Acme")).toBeTruthy();
    expect(within(table).getByText("Globex")).toBeTruthy();
    expect(within(table).getByText("Paused")).toBeTruthy();
    expect(within(table).getByText("byo.minimum_posture")).toBeTruthy();
    expect(within(table).getByRole("link", { name: "Acme" }).getAttribute("href")).toBe("/o/acme/ai-models");
  });

  it("a loosening save asks for confirmation naming the affected organizations", async () => {
    const puts = vi.fn();
    mockFetch({
      "GET /v1/admin/settings/governance": () => json(200, governance()),
      "GET /v1/admin/settings/governance/orgs": () => json(200, orgs),
      "POST /v1/admin/settings/governance/preview": () =>
        json(200, {
          generation: 2,
          diff: [{ key: "byo.enabled", before: false, after: true, before_source: "dashboard", after_source: "dashboard" }],
          loosening: [{ key: "byo.enabled", entry_id: null, reason: "`byo.enabled` becomes less strict." }],
          affected_orgs: [{ org_id: "o1", slug: "acme", name: "Acme", changes: [] }],
          affected_orgs_total: 1,
          warnings: [],
        }),
      "PUT /v1/admin/settings/governance": () => {
        puts();
        return json(200, governance());
      },
    });
    wrap(<SettingsSectionView slug="governance" />);
    await screen.findByRole("heading", { name: "Agent tokens" });
    fireEvent.click(document.getElementById("byo.enabled-c") ?? document.getElementById("byo.enabled")!);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Allow more than before?" })).toBeTruthy();
    // A sentence, not the key: the key and its reason are under Technical details only.
    expect(within(dialog).getByTestId("loosening-sentences")).toHaveTextContent("People may run agents outside Aurora.");
    expect(within(dialog).getByTestId("loosening-sentences")).not.toHaveTextContent("byo.enabled");
    expect(within(dialog).getByText("Acme")).toBeTruthy();
    expect(puts).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Yes, save" }));
    await waitFor(() => expect(puts).toHaveBeenCalledTimes(1));
  });
});

// P5-REFINE-18 #6 (admin-1, C-1066): a refused "Check where AI keys come from" springs back to Off and says why, in place.
describe("a failed key-origin save is not kept", () => {
  it("springs the switch back, names the endpoints by display name, and stays until dismissed", async () => {
    const puts = vi.fn();
    mockFetch({
      "GET /v1/admin/settings/governance": () => json(200, governance()),
      "GET /v1/admin/settings/governance/orgs": () => json(200, orgs),
      "POST /v1/admin/settings/governance/preview": () =>
        json(422, {
          error: {
            code: "key_origin_required",
            message: "These AI endpoints need a rule for where their keys come from: Company AI gateway, Users AI gateway. They are set in Helm, so the rule is added there.",
            details: { endpoints: [{ id: "central", display_name: "Company AI gateway", source: "helm" }, { id: "users", display_name: "Users AI gateway", source: "helm" }] },
          },
        }),
      "PUT /v1/admin/settings/governance": () => {
        puts();
        return json(200, governance());
      },
    });
    wrap(<SettingsSectionView slug="governance" />);
    await screen.findByRole("heading", { name: "Agent tokens" });
    const sw = () => (document.getElementById("governance.llm.require_key_origin-c") ?? document.getElementById("governance.llm.require_key_origin")) as HTMLInputElement;
    fireEvent.click(sw());
    expect(sw().checked).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const notice = await screen.findByTestId("key-origin-refused");
    expect(notice).toHaveTextContent(
      "Not turned on. 2 AI endpoints have no key origin, so their keys couldn't be checked: Company AI gateway, Users AI gateway. These endpoints are set in Helm, so add the origin there, then try again.",
    );
    expect(notice).toHaveTextContent("Nothing changed");
    await waitFor(() => expect(sw().checked).toBe(false));
    expect(puts).not.toHaveBeenCalled();
    fireEvent.click(within(notice).getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByTestId("key-origin-refused")).toBeNull();
  });
});

describe("Org governance pages: the instance link (SET-2, C-783)", () => {
  const me = (admin: boolean) => ({ "GET /v1/me": () => json(200, { user: { id: "u", email: "a@x.io", is_instance_admin: admin } }) });

  it("links an instance-enforced value to its setting for instance admins", async () => {
    mockFetch(me(true));
    wrap(<Locked setting="governance.llm.minimum_posture" />);
    const link = await screen.findByRole("link");
    expect(link.getAttribute("href")).toBe("/admin/internet-access?tab=ai-keys#governance.llm.minimum_posture");
  });

  it("is plain text for everyone else", async () => {
    mockFetch(me(false));
    wrap(<Locked setting="governance.llm.minimum_posture" />);
    await screen.findByText("Set for you by an Aurora administrator");
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("link")).toBeNull();
  });
});
