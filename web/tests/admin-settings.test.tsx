import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { json, mockFetch } from "./support/api";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/settings",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { SettingsIndexView } from "@/components/admin/settings/settings-index-view";
import { SettingsSectionView } from "@/components/admin/settings/settings-section-view";

afterEach(() => {
  window.localStorage.clear();
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

const setting = (over: Record<string, unknown>) => ({
  key: "t.k",
  value: null,
  source: "default",
  locked: false,
  helm_key: "config.t.k",
  default: null,
  schema: schema("string"),
  list: null,
  id_field: null,
  entries: null,
  secret: null,
  ...over,
});

/** A field in words (C-972): the label is written, never derived from the key. */
const fld = (key: string, label: string, kind: string, over: Record<string, unknown> = {}) => ({
  key,
  label,
  help: `${label}, in a sentence.`,
  kind,
  choices: null,
  default_display: "None",
  unit: null,
  doc: null,
  ...over,
});

function section(over: Record<string, unknown> = {}) {
  return {
    section: "settings_test",
    title: "Test section",
    description: "Exercises it.",
    generation: 3,
    updated_at: "2026-11-15T10:00:00Z",
    updated_by_label: "root@x.io",
    extra: null,
    editable: true,
    current: null,
    fields: [
      fld("t.limit", "Limit", "int"),
      fld("t.flag", "A switch", "bool"),
      fld("t.mode", "Mode", "enum", {
        choices: [
          { value: "strict", label: "Strict", description: "Nothing gets through." },
          { value: "standard", label: "Standard", description: "The usual rules." },
          { value: "open", label: "Open", description: "Everything is allowed." },
        ],
        default_display: "Standard",
      }),
      fld("t.count", "Count", "int", { default_display: "10", unit: "things" }),
      fld("t.names", "Names", "list"),
      fld("t.token", "Token", "secret"),
      fld("t.items", "Items", "keyed_list"),
    ],
    settings: [
      setting({ key: "t.limit", value: 5, source: "helm", locked: true, helm_key: "config.t.limit", default: 10, schema: schema("int", { min: 1, max: 100 }) }),
      setting({ key: "t.flag", value: false, default: false, schema: schema("bool") }),
      setting({ key: "t.mode", value: "standard", default: "standard", schema: schema("enum", { options: ["strict", "standard", "open"] }) }),
      setting({ key: "t.count", value: 20, source: "dashboard", default: 10, schema: schema("int", { min: 1, max: 100 }) }),
      setting({ key: "t.names", value: [], default: [], list: "whole", schema: schema("list", { item_kind: "string", max: 20 }) }),
      setting({ key: "t.token", value: null, schema: schema("string"), secret: { set: true, updated_at: "2026-11-15T10:00:00Z", from_env: null } }),
      setting({
        key: "t.items",
        value: [{ id: "h" }],
        default: [],
        list: "keyed",
        id_field: "id",
        schema: schema("list", { item_kind: "object" }),
        entries: [{ id: "h", value: { id: "h" }, source: "helm", locked: true, warnings: null }],
      }),
    ],
    ...over,
  };
}

const preview = (over: Record<string, unknown> = {}) => ({
  generation: 3,
  diff: [],
  loosening: [],
  affected_orgs: [],
  affected_orgs_total: 0,
  warnings: [],
  ...over,
});

describe("Admin › Settings index (SET-1)", () => {
  it("lists the sections with counts and shows one line about the Helm-set values, linking to Set in Helm", async () => {
    mockFetch({
      "GET /v1/admin/settings": () =>
        json(200, {
          sections: [
            { section: "governance", title: "Governance", description: "Floors.", generation: 0, updated_at: null, updated_by_label: null, keys_total: 4, keys_locked: 1, keys_dashboard: 2 },
            { section: "sign_in", title: "Sign-in", description: "Providers.", generation: 2, updated_at: "2026-11-15T10:00:00Z", updated_by_label: "root@x.io", keys_total: 3, keys_locked: 0, keys_dashboard: 0 },
          ],
          locked: [{ section: "governance", key: "governance.llm.minimum_posture", entry_id: null, helm_key: "config.governance.llm.minimum_posture" }],
        }),
    });
    wrap(<SettingsIndexView />);
    expect(await screen.findByText("Governance")).toBeTruthy();
    expect(screen.getByText("Sign-in")).toBeTruthy();
    expect(screen.getByText("1 in Helm")).toBeTruthy();
    expect(screen.getByText("2 in the dashboard")).toBeTruthy();
    expect(screen.getByText(/1 setting is set in Helm\./)).toBeTruthy();
    expect(screen.getByRole("link", { name: "See them on Set in Helm" }).getAttribute("href")).toBe("/admin/set-in-helm");
    // The Helm key and the long "remove it from your values" text are on Set in Helm, not here.
    expect(screen.queryByText("config.governance.llm.minimum_posture")).toBeNull();
    expect(screen.getByRole("link", { name: "Open Sign-in" }).getAttribute("href")).toBe("/admin/people?tab=sign-in");
    expect(screen.getByRole("link", { name: "Export YAML" }).getAttribute("href")).toBe("/v1/admin/settings/export?format=yaml");
  });

  it("imports: the dry run shows the diff and the ignored locked key, Apply sends the generations", async () => {
    let applied: unknown = null;
    mockFetch({
      "GET /v1/admin/settings": () => json(200, { sections: [], locked: [] }),
      "POST /v1/admin/settings/import": async (req) => {
        const b = (await req.json()) as { dry_run: boolean; generations?: unknown };
        if (b.dry_run) {
          return json(200, {
            applied: false,
            sections: [
              {
                section: "settings_test",
                generation: 4,
                diff: [{ key: "t.flag", entry_id: null, before: false, after: true, before_source: "default", after_source: "dashboard" }],
                loosening: [],
                warnings: [{ code: "locked_ignored", key: "t.limit", message: "`t.limit` is configured in Helm; the imported value equals it and is ignored." }],
              },
            ],
            errors: [],
          });
        }
        applied = b.generations;
        return json(200, { applied: true, sections: [], errors: [] });
      },
    });
    wrap(<SettingsIndexView />);
    fireEvent.click(await screen.findByRole("button", { name: "Import…" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Settings document"), { target: { value: "aurora_settings: 1\nsections: {}\n" } });
    expect((within(dialog).getByRole("button", { name: "Apply" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(within(dialog).getByRole("button", { name: "Check" }));
    expect(await within(dialog).findByText("t.flag")).toBeTruthy();
    expect(within(dialog).getByText(/imported value equals it and is ignored/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(applied).toEqual({ settings_test: 4 }));
    expect(await within(dialog).findByText(/Imported\./)).toBeTruthy();
  });
});

describe("Generic settings section form (SET-1)", () => {
  it("shows locked, dashboard and default settings, with the reset action and the write-only secret", async () => {
    mockFetch({ "GET /v1/admin/settings/settings_test": () => json(200, section()) });
    wrap(<SettingsSectionView slug="settings-test" />);
    expect(await screen.findByText("Limit")).toBeTruthy();
    // Locked: read-only text, the Helm note, no control.
    expect(screen.getByTestId("t.limit-locked-value").textContent).toBe("5");
    expect(screen.getByText(/Configured in Helm/).textContent).toContain("config.t.limit");
    expect(document.getElementById("t.limit-c")).toBeNull();
    // Dashboard: the control holds the value; Reset names the default.
    expect((document.getElementById("t.count-c") as HTMLInputElement).value).toBe("20");
    fireEvent.click(screen.getByRole("button", { name: "Reset to default (10)" }));
    expect((document.getElementById("t.count-c") as HTMLInputElement).value).toBe("10");
    // Default settings show their default in words ("Default: Standard"); the enumeration is a choice of sentences.
    expect(screen.getByTestId("t.mode-default").textContent).toBe("Default: Standard");
    expect((screen.getByRole("radio", { name: /Standard/ }) as HTMLInputElement).checked).toBe(true);
    expect(screen.getByText("The usual rules.")).toBeTruthy();
    expect(screen.queryByText("standard")).toBeNull();
    expect(screen.getByText("Count, in a sentence.")).toBeTruthy();
    expect(screen.getByText("Set")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Replace…" })).toBeTruthy();
    expect(screen.queryByDisplayValue("canary")).toBeNull();
    // The keyed list: a Helm entry is locked.
    expect(screen.getAllByText("Set by the installation").length).toBeGreaterThan(0);
  });

  it("saves via the preview, sending only the dashboard values and `keep` for the stored secret", async () => {
    let put: { generation: number; values: Record<string, unknown> } | null = null;
    mockFetch({
      "GET /v1/admin/settings/settings_test": () => json(200, section()),
      "POST /v1/admin/settings/settings_test/preview": () => json(200, preview()),
      "PUT /v1/admin/settings/settings_test": async (req) => {
        put = (await req.json()) as typeof put;
        return json(200, section({ generation: 4 }));
      },
    });
    wrap(<SettingsSectionView slug="settings-test" />);
    await screen.findByText("Limit");
    fireEvent.click(document.getElementById("t.flag-c")!);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/Saved\. Applied to every replica within seconds\./)).toBeTruthy();
    expect(put).toEqual({ generation: 3, values: { "t.flag": true, "t.count": 20, "t.token": { $secret: "keep" } } });
  });

  it("asks for confirmation when the preview reports loosening, naming the keys and organizations", async () => {
    const puts = vi.fn();
    mockFetch({
      "GET /v1/admin/settings/settings_test": () => json(200, section()),
      "POST /v1/admin/settings/settings_test/preview": () =>
        json(200, preview({
          loosening: [{ key: "t.flag", entry_id: null, reason: "`t.flag` becomes less strict." }],
          affected_orgs: [{ org_id: "o1", slug: "acme", name: "Acme", changes: [] }],
          affected_orgs_total: 1,
        })),
      "PUT /v1/admin/settings/settings_test": () => {
        puts();
        return json(200, section({ generation: 4 }));
      },
    });
    wrap(<SettingsSectionView slug="settings-test" />);
    await screen.findByText("Limit");
    fireEvent.click(document.getElementById("t.flag-c")!);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Allow more than before?" })).toBeTruthy();
    // `t.flag` has no template: the generic sentence, never the key outside Technical details.
    expect(within(dialog).getByTestId("loosening-sentences")).not.toHaveTextContent("t.flag");
    expect(within(dialog).getByText("Acme")).toBeTruthy();
    expect(puts).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Yes, save" }));
    await waitFor(() => expect(puts).toHaveBeenCalledTimes(1));
  });

  it("handles 409 stale_revision and 422 field errors, with no re-authentication step (FIX-157)", async () => {
    let n = 0;
    mockFetch({
      "GET /v1/admin/settings/settings_test": () => json(200, section()),
      "POST /v1/admin/settings/settings_test/preview": () => {
        n += 1;
        if (n === 1) return json(409, { error: { code: "stale_revision", message: "stale", details: { current_generation: 9 } } });
        return json(422, { error: { code: "settings_invalid", message: "bad", details: { fields: [{ path: "/t.count", code: "invalid_value", message: "must be between 1 and 100" }, { path: "/other.key", code: "x", message: "needs t.flag" }] } } });
      },
    });
    wrap(<SettingsSectionView slug="settings-test" />);
    await screen.findByText("Limit");
    fireEvent.click(document.getElementById("t.flag-c")!);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/Someone changed these settings meanwhile/)).toBeTruthy();
    // The typed value is kept until Reload.
    expect((document.getElementById("t.flag-c") as HTMLInputElement).checked).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("must be between 1 and 100")).toBeTruthy();
    expect(screen.getByText(/needs t\.flag/)).toBeTruthy();
    expect(screen.queryByText(/sign in again/i)).toBeNull();
  });
});

// P5-REFINE-18 #16 (setup-6): a server address is one line, not a text area.
describe("single-line string settings", () => {
  it("renders a 512-character address as an input and a 20 000-character document as a text area", async () => {
    mockFetch({
      "GET /v1/admin/settings/settings_test": () =>
        json(
          200,
          section({
            fields: [fld("t.url", "Mail server address", "string"), fld("t.doc", "Template", "string")],
            settings: [
              setting({ key: "t.url", value: "smtp://mail.example.com:587", source: "dashboard", schema: schema("string", { max_len: 512 }) }),
              setting({ key: "t.doc", value: "x", source: "dashboard", schema: schema("string", { max_len: 20000 }) }),
            ],
          }),
        ),
    });
    wrap(<SettingsSectionView slug="settings-test" />);
    await screen.findByText("Mail server address");
    expect(document.getElementById("t.url-c")?.tagName).toBe("INPUT");
    expect(document.getElementById("t.doc-c")?.tagName).toBe("TEXTAREA");
  });
});
