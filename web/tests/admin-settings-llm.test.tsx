import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/settings/llm",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { SettingsSectionView } from "@/components/admin/settings/settings-section-view";

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

const PROVIDERS = ["litellm", "anthropic", "openai", "openai_compatible", "vertex"];
const central = { id: "central", provider: "litellm", base_url: "https://llm.example.test", default_model: "m" };
const team = { id: "team", provider: "openai_compatible", base_url: "https://team.example.test/v1" };

interface Opts {
  enabled?: string[];
  enabledLocked?: boolean;
  prefLocked?: boolean;
  entries?: { id: string; value: Record<string, unknown>; source: "helm" | "dashboard"; warnings?: string[] }[];
  /** `extra.endpoint_harnesses` (C-811): the enabled harnesses that speak each endpoint's API. */
  harnesses?: Record<string, string[]>;
}

function llm(o: Opts = {}) {
  const enabled = o.enabled ?? ["litellm", "anthropic"];
  const entries: NonNullable<Opts["entries"]> = o.entries ?? [
    { id: "central", value: central, source: "helm" },
    { id: "team", value: team, source: "dashboard" },
  ];
  return {
    section: "llm",
    title: "LLM endpoints",
    description: "Providers and endpoints agents may reach.",
    generation: 4,
    updated_at: "2026-11-15T10:00:00Z",
    updated_by_label: "root@x.io",
    extra: { providers: PROVIDERS, ...(o.harnesses ? { endpoint_harnesses: o.harnesses } : {}) },
    settings: [
      {
        key: "llm.enabled_providers",
        value: enabled,
        source: o.enabledLocked ? "helm" : "default",
        locked: Boolean(o.enabledLocked),
        helm_key: "config.llm.enabled_providers",
        default: ["litellm", "anthropic"],
        schema: schema("list", { item_kind: "string" }),
        list: "whole",
        id_field: null,
        entries: null,
        secret: null,
      },
      {
        key: "llm.provider_preference",
        value: ["litellm", "anthropic"],
        source: o.prefLocked ? "helm" : "default",
        locked: Boolean(o.prefLocked),
        helm_key: "config.llm.provider_preference",
        default: ["litellm", "anthropic"],
        schema: schema("list", { item_kind: "string" }),
        list: "whole",
        id_field: null,
        entries: null,
        secret: null,
      },
      {
        key: "llm.endpoints",
        value: entries.map((e) => e.value),
        source: entries.some((e) => e.source === "helm") ? "helm" : entries.length ? "dashboard" : "default",
        locked: false,
        helm_key: "config.llm.endpoints",
        default: [],
        schema: schema("list", { item_kind: "object" }),
        list: "keyed",
        id_field: "id",
        entries: entries.map((e) => ({ id: e.id, value: e.value, source: e.source, locked: e.source === "helm", warnings: e.warnings ?? null })),
        secret: null,
      },
    ],
  };
}

const preview = (over: Record<string, unknown> = {}) => ({
  generation: 4,
  diff: [],
  loosening: [],
  affected_orgs: [],
  affected_orgs_total: 0,
  warnings: [],
  ...over,
});

async function open(routes: Record<string, Handler> = {}, o: Opts = {}) {
  const fn = mockFetch({ "GET /v1/admin/settings/llm": () => json(200, llm(o)), ...routes });
  wrap(<SettingsSectionView slug="llm" />);
  await screen.findByRole("heading", { name: "Providers" });
  return fn;
}

/** (P5-REFINE-18 #9) Adding is two steps: skip the test and go to "Models and roles". */
function toModelsStep(dialog: HTMLElement) {
  fireEvent.click(within(dialog).getByRole("button", { name: "Skip the test" }));
  fireEvent.click(within(dialog).getByRole("button", { name: "Next: models and roles" }));
}

const bodyOf = async (req: Request) => (await req.clone().json()) as { generation: number; values: Record<string, unknown> };

describe("Admin › Settings › LLM endpoints (SET-3)", () => {
  it("lists the providers of this release as checkboxes and saves the enabled list", async () => {
    let put: { values: Record<string, unknown> } | null = null;
    await open({
      "POST /v1/admin/settings/llm/preview": () => json(200, preview()),
      "PUT /v1/admin/settings/llm": async (req) => {
        put = await bodyOf(req);
        return json(200, llm({ enabled: ["litellm", "anthropic", "openai"] }));
      },
    });
    const group = screen.getByRole("group", { name: "Enabled providers" });
    for (const name of ["LiteLLM", "Anthropic", "OpenAI", "OpenAI-compatible server", "Google Vertex AI"]) {
      expect(within(group).getByRole("checkbox", { name })).toBeTruthy();
    }
    expect((within(group).getByRole("checkbox", { name: "LiteLLM" }) as HTMLInputElement).checked).toBe(true);
    expect((within(group).getByRole("checkbox", { name: "OpenAI" }) as HTMLInputElement).checked).toBe(false);
    fireEvent.click(within(group).getByRole("checkbox", { name: "OpenAI" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(put).not.toBeNull());
    expect(put!.values["llm.enabled_providers"]).toEqual(["litellm", "anthropic", "openai"]);
    // The preference is untouched: appended providers need no entry.
    expect("llm.provider_preference" in put!.values).toBe(false);
  });

  it("reorders the preference with buttons and drops a provider that is switched off", async () => {
    const puts: Record<string, unknown>[] = [];
    await open({
      "POST /v1/admin/settings/llm/preview": () => json(200, preview()),
      "PUT /v1/admin/settings/llm": async (req) => {
        puts.push((await bodyOf(req)).values);
        return json(200, llm());
      },
    });
    const list = screen.getByRole("list", { name: "Preference order" });
    const items = () => within(list).getAllByRole("listitem").map((li) => li.textContent ?? "");
    expect(items()[0]).toContain("LiteLLM");
    expect(items()[1]).toContain("Anthropic");
    expect((within(list).getByRole("button", { name: "Move LiteLLM up" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(within(list).getByRole("button", { name: "Move Anthropic up" }));
    expect(items()[0]).toContain("Anthropic");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts.length).toBe(1));
    expect(puts[0]!["llm.provider_preference"]).toEqual(["anthropic", "litellm"]);
    // Switching a provider off removes it from the order (the server refuses a preference naming it).
    fireEvent.click(screen.getByRole("checkbox", { name: "Anthropic" }));
    expect(items().some((t) => t.includes("Anthropic"))).toBe(false);
  });

  it("shows a provider list and a preference order that Helm sets as read-only, with their Helm keys", async () => {
    await open({}, { enabledLocked: true, prefLocked: true, enabled: ["litellm", "anthropic"] });
    expect(screen.queryByRole("group", { name: "Enabled providers" })).toBeNull();
    expect(screen.queryByRole("list", { name: "Preference order" })).toBeNull();
    expect(screen.getByTestId("llm.enabled_providers-locked-value").textContent).toBe("LiteLLM, Anthropic");
    expect(screen.getByTestId("llm.provider_preference-locked-value").textContent).toContain("1. LiteLLM");
    expect(screen.getByText("config.llm.enabled_providers")).toBeTruthy();
    expect(screen.getByText("config.llm.provider_preference")).toBeTruthy();
  });

  it("shows a Helm endpoint read-only beside an editable dashboard endpoint", async () => {
    await open();
    const list = screen.getByRole("list", { name: "Endpoints" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(within(rows[0]!).getByText("Set by the installation")).toBeTruthy();
    expect(within(rows[0]!).queryByRole("button", { name: /Edit|Remove/ })).toBeNull();
    expect(within(rows[1]!).getByRole("button", { name: "Edit team" })).toBeTruthy();
    expect(within(rows[1]!).getByRole("button", { name: "Remove team" })).toBeTruthy();
    fireEvent.click(within(rows[0]!).getByRole("button", { name: "View central" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/Configured in Helm/)).toBeTruthy();
    expect(within(dialog).getByText("config.llm.endpoints")).toBeTruthy();
    expect(within(dialog).getByText("https://llm.example.test")).toBeTruthy();
    expect(within(dialog).queryByRole("button", { name: "Done" })).toBeNull();
  });

  it("shows each provider's own fields in the endpoint dialog", async () => {
    await open({}, { enabled: ["litellm", "anthropic", "openai", "openai_compatible", "vertex"] });
    fireEvent.click(screen.getByRole("button", { name: "Add endpoint…" }));
    const dialog = await screen.findByRole("dialog");
    const select = within(dialog).getByLabelText("Provider") as HTMLSelectElement;
    const has = (label: string) => within(dialog).queryByLabelText(label) !== null;
    // litellm (the first provider): the address and allowed paths (under Advanced options)
    expect(has("Address")).toBe(true);
    expect(has("Allowed paths")).toBe(true);
    expect(has("OpenAI organization")).toBe(false);
    fireEvent.change(select, { target: { value: "openai" } });
    expect(has("OpenAI organization")).toBe(true);
    expect(has("OpenAI project")).toBe(true);
    expect(has("Address")).toBe(false);
    fireEvent.change(select, { target: { value: "openai_compatible" } });
    expect(has("Key header")).toBe(true);
    expect(has("Models path")).toBe(true);
    expect(has("Address")).toBe(true);
    fireEvent.change(select, { target: { value: "vertex" } });
    expect(has("Project id")).toBe(true);
    expect(has("Location")).toBe(true);
    // no allowed_paths for Vertex (C-800)
    expect(has("Allowed paths")).toBe(false);
    expect(has("Address")).toBe(false);
    // the key origin method follows the provider
    const methods = Array.from((within(dialog).getByLabelText("Method") as HTMLSelectElement).options).map((o) => o.value);
    expect(methods).toEqual(["", "provider_account"]);
    // default model and models are both required for Vertex: they are on the second step
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Vertex one" } });
    toModelsStep(dialog);
    expect(within(dialog).getAllByText("Required for this provider.", { exact: false })).toHaveLength(2);
  });

  it("adds in two steps: Connect and test (the result beside the button, Next after it passed or was skipped), then Models and roles (P5-REFINE-18 #9)", async () => {
    await open({
      "POST /v1/admin/settings/llm/endpoint-test": () => json(200, { reachable: true, status: 200, error_code: null, models: ["a", "b", "c"] }),
    });
    fireEvent.click(screen.getByRole("button", { name: "Add endpoint…" }));
    const dialog = await screen.findByRole("dialog", { name: "Add AI endpoint" });
    expect(within(dialog).getByRole("list", { name: "Steps" }).textContent).toContain("Connect and test");
    expect(within(dialog).getByRole("list", { name: "Steps" }).textContent).toContain("Models and roles");
    // The id is made from the name until it is edited; Next waits for a test (or skipping it).
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Users AI gateway" } });
    expect(within(dialog).getByLabelText("Endpoint id")).toHaveValue("users-ai-gateway");
    fireEvent.change(within(dialog).getByLabelText("Address"), { target: { value: "https://llm.acme.example/v1" } });
    const next = within(dialog).getByRole("button", { name: "Next: models and roles" }) as HTMLButtonElement;
    expect(next.disabled).toBe(true);
    expect(within(dialog).queryByLabelText("Default model")).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Test connection" }));
    const row = within(dialog).getByTestId("endpoint-test-row");
    expect((await within(row).findByRole("status", { name: "Connection test result" })).textContent).toContain("Connected. 3 models found.");
    expect(next.disabled).toBe(false);
    fireEvent.click(next);
    expect(await within(dialog).findByLabelText("Default model")).toBeTruthy();
    expect(within(dialog).queryByLabelText("Address")).toBeNull();
    expect(within(dialog).getByRole("button", { name: "Done" })).toBeTruthy();
  });

  it("adds a dashboard endpoint: Done puts it in the save, with only its provider's keys", async () => {
    let put: Record<string, unknown> | null = null;
    await open({
      "POST /v1/admin/settings/llm/preview": () => json(200, preview()),
      "PUT /v1/admin/settings/llm": async (req) => {
        put = (await bodyOf(req)).values;
        return json(200, llm());
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add endpoint…" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Endpoint id"), { target: { value: "gw" } });
    fireEvent.change(within(dialog).getByLabelText("Address"), { target: { value: "https://gw.example.test" } });
    fireEvent.change(within(dialog).getByLabelText("Method"), { target: { value: "endpoint" } });
    toModelsStep(dialog);
    fireEvent.change(within(dialog).getByLabelText("Allowed models"), { target: { value: "m1\nm2" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // Done does not save the page.
    expect(put).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(put).not.toBeNull());
    const list = put!["llm.endpoints"] as Record<string, unknown>[];
    expect(list.map((e) => e.id)).toEqual(["team", "gw"]);
    expect(list[1]).toEqual({
      id: "gw",
      provider: "litellm",
      base_url: "https://gw.example.test",
      allowed_models: ["m1", "m2"],
      key_origin: { method: "endpoint" },
    });
  });

  it("names an id that a Helm endpoint holds, in the dialog and from the server", async () => {
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Add endpoint…" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Endpoint id"), { target: { value: "central" } });
    fireEvent.change(within(dialog).getByLabelText("Address"), { target: { value: "https://x.example.test" } });
    expect(within(dialog).getByText("`central` is defined in Helm; choose another id")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Skip the test" }));
    expect((within(dialog).getByRole("button", { name: "Next: models and roles" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows a server's field error on the endpoint's row", async () => {
    await open({
      "POST /v1/admin/settings/llm/preview": () => json(200, preview()),
      "PUT /v1/admin/settings/llm": () =>
        json(422, {
          error: {
            code: "settings_invalid",
            message: "Some settings are not valid.",
            details: { fields: [{ path: "/llm.endpoints/team/base_url", code: "base_url_invalid", message: "must be https" }] },
          },
        }),
    });
    // make the page dirty: switch a provider on
    fireEvent.click(screen.getByRole("checkbox", { name: "OpenAI" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const row = (await screen.findByText("Needs attention")).closest("li")!;
    expect(within(row).getByText("must be https", { exact: false })).toBeTruthy();
  });

  it("tests the connection from the dialog and words the result", async () => {
    const bodies: unknown[] = [];
    await open({
      "POST /v1/admin/settings/llm/endpoint-test": async (req) => {
        bodies.push(await req.clone().json());
        return bodies.length === 1
          ? json(200, { reachable: true, status: 401, error_code: null, models: null })
          : json(200, { reachable: false, status: null, error_code: "destination_refused", models: null });
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Edit team" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Test connection" }));
    const ok = await within(dialog).findByRole("status", { name: "Connection test result" });
    expect(ok.textContent).toContain("Connected.");
    expect(ok.textContent).toContain("It answered HTTP 401, as expected without a key.");
    // The request carries the entry, never a key.
    expect(JSON.stringify(bodies[0])).toContain("team.example.test");
    expect(JSON.stringify(bodies[0])).not.toMatch(/api_key|secret|token/);
    fireEvent.click(within(dialog).getByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(within(dialog).getByRole("status", { name: "Connection test result" }).textContent).toContain("Aurora can't reach private addresses. Ask whoever installed Aurora to allow it."));
  });

  it("asks for confirmation naming the organizations whose keys stop working", async () => {
    const puts = vi.fn();
    await open({
      "POST /v1/admin/settings/llm/preview": () =>
        json(
          200,
          preview({
            affected_orgs: [
              {
                org_id: "o1",
                slug: "acme",
                name: "Acme",
                changes: [{ key: "llm.endpoints", before: { usable_keys: 3 }, after: { usable_keys: 1 } }],
              },
            ],
            affected_orgs_total: 1,
            warnings: [
              { code: "secrets_become_non_permitted", key: "llm.endpoints", message: "2 LLM keys in 1 organization would stop being usable." },
            ],
          }),
        ),
      "PUT /v1/admin/settings/llm": () => {
        puts();
        return json(200, llm());
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Remove team" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Acme — 2 AI keys stop working")).toBeTruthy();
    expect(within(dialog).getByText(/would stop being usable/)).toBeTruthy();
    expect(puts).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Yes, save" }));
    await waitFor(() => expect(puts).toHaveBeenCalledTimes(1));
  });

  it("shows the harnesses each endpoint serves as badges, and a notice for one that serves none (D-260(a))", async () => {
    await open({}, { harnesses: { central: ["claude_code", "codex"], team: [] } });
    const rows = within(screen.getByRole("list", { name: "Endpoints" })).getAllByRole("listitem");
    const line = within(rows[0]!).getByLabelText("Harnesses this endpoint serves");
    expect(line.textContent).toBe("Works with Claude Code, Codex");
    expect(within(rows[0]!).queryByText(/No enabled harness speaks this API/)).toBeNull();
    expect(within(rows[1]!).getByText("No enabled harness speaks this API. Put a LiteLLM endpoint in front to translate.")).toBeTruthy();
  });

  it("marks a dashboard endpoint on a private address the gateway refuses, linking to Limits (C-813)", async () => {
    await open(
      {},
      {
        entries: [
          { id: "central", value: central, source: "helm" },
          { id: "team", value: team, source: "dashboard", warnings: ["needs_allowed_private"] },
        ],
      },
    );
    const rows = within(screen.getByRole("list", { name: "Endpoints" })).getAllByRole("listitem");
    expect(within(rows[0]!).queryByText("Private address not allowed")).toBeNull();
    const badge = within(rows[1]!).getByText("Private address not allowed");
    const link = badge.closest("a")!;
    expect(link.getAttribute("href")).toMatch(/^\/admin\/platform\?tab=limits#gateway\.allowed_private$/);
  });

  it("words http_needs_private_host and endpoint_host_claimed on the endpoint's base URL", async () => {
    await open({
      "POST /v1/admin/settings/llm/preview": () => json(200, preview()),
      "PUT /v1/admin/settings/llm": () =>
        json(422, {
          error: {
            code: "settings_invalid",
            message: "Some settings are not valid.",
            details: {
              fields: [
                { path: "/llm.endpoints/team/base_url", code: "http_needs_private_host", message: "plain http is only allowed to a private address that gateway.allowed_private opens" },
                { path: "/llm.endpoints/team/base_url", code: "endpoint_host_claimed", message: "the host belongs to the app `jira`", app_id: "jira" },
              ],
            },
          },
        }),
    });
    fireEvent.click(screen.getByRole("checkbox", { name: "OpenAI" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const row = (await screen.findByText("Needs attention")).closest("li")!;
    expect(within(row).getByText(/Aurora can't reach private addresses\. Ask whoever installed Aurora to allow it\./)).toBeTruthy();
    // The dialog puts the message on the base URL field itself.
    fireEvent.click(within(row).getByRole("button", { name: "Edit team" }));
    const dialog = await screen.findByRole("dialog");
    const base = within(dialog).getByLabelText("Address");
    expect(base.getAttribute("aria-invalid")).toBe("true");
    expect(within(dialog).getByText(/Aurora can't reach private addresses\. Ask whoever installed Aurora to allow it\./)).toBeTruthy();
  });

  it("shows the endpoint_resolves_private warning in the save confirmation (C-813)", async () => {
    const puts = vi.fn();
    await open({
      "POST /v1/admin/settings/llm/preview": () =>
        json(
          200,
          preview({
            warnings: [
              {
                code: "endpoint_resolves_private",
                key: "llm.endpoints",
                entry_id: "team",
                message: "The host of endpoint `team` resolves to a private, loopback or link-local address.",
              },
            ],
          }),
        ),
      "PUT /v1/admin/settings/llm": () => {
        puts();
        return json(200, llm());
      },
    });
    fireEvent.click(screen.getByRole("checkbox", { name: "OpenAI" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/resolves to a private, loopback or link-local address/)).toBeTruthy();
    expect(puts).not.toHaveBeenCalled();
  });

  it("hides allowed_paths for Bedrock and Vertex and shows each provider's own fields", async () => {
    await open({}, { enabled: ["litellm", "anthropic", "vertex", "openai_compatible"] });
    fireEvent.click(screen.getByRole("button", { name: "Add endpoint…" }));
    let dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Provider"), { target: { value: "vertex" } });
    expect(within(dialog).getByLabelText("Project id")).toBeTruthy();
    expect(within(dialog).getByLabelText("Location")).toBeTruthy();
    expect(within(dialog).queryByLabelText("Allowed paths")).toBeNull();
    fireEvent.change(within(dialog).getByLabelText("Provider"), { target: { value: "openai_compatible" } });
    expect(within(dialog).getByLabelText("Key header")).toBeTruthy();
    expect(within(dialog).getByLabelText("Models path")).toBeTruthy();
    expect(within(dialog).getByLabelText("Allowed paths")).toBeTruthy();
    dialog = screen.getByRole("dialog");
    expect(dialog).toBeTruthy();
  });

  // (phase 5, P5-MODELS-3, C-875)
  describe("model roles and model details", () => {
    const glm = {
      id: "glm", provider: "litellm", base_url: "https://glm.example.test", models: ["glm-4.6", "glm-4.5-air"],
      model_roles: { large: "glm-4.6", small: "glm-4.5-air" },
      model_info: { "glm-4.6": { context_window: 200000, capabilities: ["thinking", "effort"] } },
    };

    it("sets roles and model details in the endpoint dialog and saves them as the C-875 keys", async () => {
      let put: Record<string, unknown> | null = null;
      await open({
        "POST /v1/admin/settings/llm/preview": () => json(200, preview()),
        "PUT /v1/admin/settings/llm": async (req) => {
          put = (await bodyOf(req)).values;
          return json(200, llm());
        },
      });
      fireEvent.click(screen.getByRole("button", { name: "Add endpoint…" }));
      const dialog = await screen.findByRole("dialog");
      fireEvent.change(within(dialog).getByLabelText("Endpoint id"), { target: { value: "gw" } });
      fireEvent.change(within(dialog).getByLabelText("Address"), { target: { value: "https://gw.example.test" } });
      toModelsStep(dialog);
      fireEvent.change(within(dialog).getByLabelText("Models"), { target: { value: "glm-4.6\nglm-4.5-air" } });
      // a non-Anthropic endpoint without roles carries the notice until a role is set
      expect(within(dialog).getByTestId("no-roles-notice")).toHaveTextContent("Set the roles.");
      fireEvent.change(within(dialog).getByLabelText("Strongest model"), { target: { value: "glm-4.6" } });
      fireEvent.change(within(dialog).getByLabelText("Fastest model"), { target: { value: " glm-4.5-air " } });
      expect(within(dialog).queryByTestId("no-roles-notice")).toBeNull();
      fireEvent.click(within(dialog).getByRole("button", { name: "Add model details" }));
      fireEvent.change(within(dialog).getByLabelText("Model id 1"), { target: { value: "glm-4.6" } });
      fireEvent.change(within(dialog).getByLabelText("Context window 1"), { target: { value: "200000" } });
      fireEvent.click(within(dialog).getByRole("checkbox", { name: "Thinking" }));
      fireEvent.click(within(dialog).getByRole("checkbox", { name: "Effort" }));
      fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(put).not.toBeNull());
      const gw = (put!["llm.endpoints"] as Record<string, unknown>[]).find((e) => e.id === "gw")!;
      expect(gw.model_roles).toEqual({ large: "glm-4.6", small: "glm-4.5-air" });
      expect(gw.model_info).toEqual({ "glm-4.6": { context_window: 200000, capabilities: ["effort", "thinking"] } });
    });

    it("edits an endpoint's roles and details in place, and clearing them removes the keys", async () => {
      let put: Record<string, unknown> | null = null;
      await open(
        {
          "POST /v1/admin/settings/llm/preview": () => json(200, preview()),
          "PUT /v1/admin/settings/llm": async (req) => {
            put = (await bodyOf(req)).values;
            return json(200, llm());
          },
        },
        { entries: [{ id: "glm", value: glm, source: "dashboard" }] },
      );
      expect(screen.getByTestId("endpoint-roles")).toHaveTextContent("Roles: Strongest model glm-4.6 · Fastest model glm-4.5-air");
      fireEvent.click(screen.getByRole("button", { name: "Edit glm" }));
      const dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByLabelText("Strongest model")).toHaveValue("glm-4.6");
      expect(within(dialog).getByLabelText("Context window 1")).toHaveValue("200000");
      expect(within(dialog).getByRole("checkbox", { name: "Thinking" })).toBeChecked();
      expect(within(dialog).getByRole("checkbox", { name: "Interleaved thinking" })).not.toBeChecked();
      fireEvent.change(within(dialog).getByLabelText("Strongest model"), { target: { value: "" } });
      fireEvent.change(within(dialog).getByLabelText("Fastest model"), { target: { value: "" } });
      fireEvent.click(within(dialog).getByRole("button", { name: "Remove details of model 1" }));
      fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(put).not.toBeNull());
      const out = (put!["llm.endpoints"] as Record<string, unknown>[])[0]!;
      expect(out).not.toHaveProperty("model_roles");
      expect(out).not.toHaveProperty("model_info");
    });

    it("checks ids and the context window on the client and keeps Done disabled", async () => {
      await open();
      fireEvent.click(screen.getByRole("button", { name: "Add endpoint…" }));
      const dialog = await screen.findByRole("dialog");
      fireEvent.change(within(dialog).getByLabelText("Endpoint id"), { target: { value: "gw" } });
      fireEvent.change(within(dialog).getByLabelText("Address"), { target: { value: "https://gw.example.test" } });
      toModelsStep(dialog);
      const done = within(dialog).getByRole("button", { name: "Done" }) as HTMLButtonElement;
      expect(done.disabled).toBe(false);
      fireEvent.change(within(dialog).getByLabelText("Everyday model"), { target: { value: "bad id!" } });
      expect(within(dialog).getByText("Letters, digits and . _ : / @ - only, up to 200 characters.")).toBeTruthy();
      expect(done.disabled).toBe(true);
      fireEvent.change(within(dialog).getByLabelText("Everyday model"), { target: { value: "ok-id" } });
      fireEvent.click(within(dialog).getByRole("button", { name: "Add model details" }));
      fireEvent.change(within(dialog).getByLabelText("Model id 1"), { target: { value: "m" } });
      fireEvent.change(within(dialog).getByLabelText("Context window 1"), { target: { value: "12" } });
      expect(within(dialog).getByText(/A whole number from 1,000 to 10,000,000/)).toBeTruthy();
      expect(done.disabled).toBe(true);
      fireEvent.change(within(dialog).getByLabelText("Context window 1"), { target: { value: "" } });
      expect(done.disabled).toBe(false);
    });

    it("shows the server's value_invalid on the roles and the details fields", async () => {
      await open(
        {
          "POST /v1/admin/settings/llm/preview": () => json(200, preview()),
          "PUT /v1/admin/settings/llm": () =>
            json(422, {
              error: {
                code: "settings_invalid",
                message: "Some settings are not valid.",
                details: {
                  fields: [
                    { path: "/llm.endpoints/glm/model_roles", code: "value_invalid", message: "large must be one of the endpoint's models" },
                    { path: "/llm.endpoints/glm/model_info", code: "value_invalid", message: "context_window out of range" },
                  ],
                },
              },
            }),
        },
        { entries: [{ id: "glm", value: glm, source: "dashboard" }] },
      );
      fireEvent.click(screen.getByRole("checkbox", { name: "OpenAI" }));
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      await screen.findByText("Needs attention");
      fireEvent.click(screen.getByRole("button", { name: "Edit glm" }));
      const dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByText("large must be one of the endpoint's models")).toBeTruthy();
      expect(within(dialog).getByText("context_window out of range")).toBeTruthy();
    });

    it("shows an endpoint's role warning once, with its fix: Set roles on a dashboard row, the Helm path on a Helm row, nothing on one without a warning (C-1034, D-385)", async () => {
      await open(
        {
          "GET /v1/admin/llm-endpoint-warnings": () =>
            json(200, [
              { endpoint_id: "central", roles: [{ role: "large", warning: "role_unset" }, { role: "medium", warning: "role_unset" }, { role: "small", warning: "role_unset" }] },
              { endpoint_id: "glm", roles: [{ role: "small", warning: "role_unset" }] },
            ]),
        },
        {
          entries: [
            { id: "central", value: central, source: "helm" },
            { id: "glm", value: { id: "glm", provider: "openai_compatible", base_url: "https://glm.test/v1" }, source: "dashboard" },
            { id: "anth", value: { id: "anth", provider: "anthropic" }, source: "dashboard" },
          ],
        },
      );
      const rows = within(screen.getByRole("list", { name: "Endpoints" })).getAllByRole("listitem");
      // Helm: neutral, says where, no button, no chip beyond "Set in Helm".
      const helm = await within(rows[0]!).findByTestId("endpoint-role-warning");
      // (P5-REFINE-20 #16) The sentence says where in words; the key is under Technical details.
      const sentence = within(helm).getByText(/Agents may ask for a model/);
      expect(sentence.textContent).toContain("Set the Strongest model, Everyday model and Fastest model roles in your Helm values.");
      expect(sentence.textContent).not.toContain("llm.endpoints");
      const fold = within(helm).getByText("Technical details").closest("details")!;
      expect(fold.textContent).toContain("Helm key: llm.endpoints.central.roles");
      expect(within(helm).queryByRole("button")).toBeNull();
      expect(within(rows[0]!).getByText("LiteLLM gateway")).toBeTruthy();
      // Dashboard: one line with Set roles, which opens the editor.
      const own = await within(rows[1]!).findByTestId("endpoint-role-warning");
      expect(own.textContent).toContain("set the Fastest model role.");
      fireEvent.click(within(own).getByRole("button", { name: "Set roles of glm" }));
      expect(await screen.findByRole("dialog")).toBeTruthy();
      fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
      // No warning from the server: nothing, and Anthropic's roles come from the provider.
      expect(within(rows[2]!).queryByTestId("endpoint-role-warning")).toBeNull();
      expect(within(rows[2]!).getByText("Roles from the provider")).toBeTruthy();
      // The per-endpoint "Agents on this endpoint will ask for Claude model names" notice is gone.
      expect(screen.queryByText(/Agents on this endpoint will ask for Claude model names/)).toBeNull();
    });
  });
});
