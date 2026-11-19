import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/settings/sign-in",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { SettingsSectionView } from "@/components/admin/settings/settings-section-view";

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- request bodies are inspected loosely
type Json = Record<string, any>;
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

const schema = (kind: string, extra: Record<string, unknown> = {}) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null, ...extra });
const base = { locked: false, helm_key: "config.x", list: null, id_field: null, entries: null, secret: null, default: null };

const helmProvider = { id: "helm", display_name: "Helm SSO", issuer: "https://helm.example.test", client_id: "c", client_secret_env: "HELM_SECRET" };
const corp = {
  id: "corp",
  display_name: "Corp",
  issuer: "https://corp.example.test",
  client_id: "corp-client",
  client_secret: { $secret: { set: true } },
  role_claim: "roles",
  role_mappings: [{ value: "ops", instance_admin: false, org: { slug: "acme", role: "member" } }],
};

function signIn() {
  return {
    section: "sign_in",
    title: "Sign-in",
    description: "Sign-up, sessions and identity providers.",
    generation: 7,
    updated_at: "2026-11-15T10:00:00Z",
    updated_by_label: "root@x.io",
    extra: { providers_from_helm: 1, signed_in_with: "helm" },
    settings: [
      { ...base, key: "auth.signup_mode", value: "invite_only", source: "default", default: "invite_only", schema: schema("enum", { options: ["invite_only", "sso_jit", "open"] }) },
      { ...base, key: "auth.org_creation", value: "instance_admins", source: "default", default: "instance_admins", schema: schema("enum", { options: ["instance_admins", "anyone"] }) },
      { ...base, key: "auth.session_idle_secs", value: 3600, source: "default", default: 3600, schema: schema("duration_secs", { min: 60 }) },
      { ...base, key: "auth.session_absolute_secs", value: 86400, source: "default", default: 86400, schema: schema("duration_secs", { min: 60 }) },
      { ...base, key: "auth.cli_key_ttl_days", value: 90, source: "default", default: 90, schema: schema("int", { min: 1 }) },
      {
        ...base,
        key: "auth.oidc_providers",
        value: [helmProvider, corp],
        source: "helm",
        helm_key: "config.auth.oidc_providers",
        default: [],
        list: "keyed",
        id_field: "id",
        schema: schema("list", { item_kind: "object" }),
        entries: [
          { id: "helm", value: helmProvider, source: "helm", locked: true, warnings: null },
          { id: "corp", value: corp, source: "dashboard", locked: false, warnings: null },
        ],
      },
    ],
  };
}

const preview = (over: Record<string, unknown> = {}) => ({ generation: 7, diff: [], loosening: [], affected_orgs: [], affected_orgs_total: 0, warnings: [], ...over });
const bodyOf = async (req: Request) => (await req.clone().json()) as Json;

async function open(routes: Record<string, Handler> = {}) {
  const fn = mockFetch({ "GET /v1/admin/settings/sign_in": () => json(200, signIn()), ...routes });
  wrap(<SettingsSectionView slug="sign-in" />);
  await screen.findByRole("list", { name: "Identity providers" });
  return fn;
}

describe("Admin › Settings › Sign-in (SET-4)", () => {
  it("lists a Helm provider read-only beside a dashboard provider, with no secret shown", async () => {
    await open();
    const list = screen.getByRole("list", { name: "Identity providers" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(within(rows[0]!).getByText("Managed by Helm")).toBeTruthy();
    expect(within(rows[0]!).getByText("You signed in with this")).toBeTruthy();
    expect(within(rows[0]!).queryByRole("button", { name: /Edit|Remove/ })).toBeNull();
    expect(within(rows[1]!).getByRole("button", { name: "Edit corp" })).toBeTruthy();
    fireEvent.click(within(rows[0]!).getByRole("button", { name: "View helm" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/Configured in Helm/)).toBeTruthy();
    expect(within(dialog).getByText("config.auth.oidc_providers")).toBeTruthy();
    expect(within(dialog).queryByRole("button", { name: "Done" })).toBeNull();
  });

  it("the dialog has Basics, E-mail, Roles, Groups and JIT, with a write-only secret and a discovery test", async () => {
    let tested: Json | null = null;
    await open({
      "POST /v1/admin/settings/sign_in/provider-test": async (req) => {
        tested = await bodyOf(req);
        return json(200, { ok: true, issuer: "https://new.example.test", authorization_endpoint: "https://new.example.test/auth", token_endpoint: "https://new.example.test/token", jwks_uri: "https://new.example.test/jwks" });
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add provider…" }));
    const dialog = await screen.findByRole("dialog");
    for (const t of ["Basics", "E-mail", "Roles", "Groups", "Just-in-time sign-up"]) {
      expect(within(dialog).getByText(t, { selector: "legend" })).toBeTruthy();
    }
    const secret = within(dialog).getByLabelText("Client secret: new value") as HTMLInputElement;
    expect(secret.type).toBe("password");
    fireEvent.change(within(dialog).getByLabelText("Provider id"), { target: { value: "new" } });
    fireEvent.change(within(dialog).getByLabelText("Issuer"), { target: { value: "https://new.example.test" } });
    fireEvent.change(within(dialog).getByLabelText("Client id"), { target: { value: "cid" } });
    fireEvent.change(secret, { target: { value: "s3cret-canary" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Test discovery" }));
    const res = await within(dialog).findByRole("status", { name: "Discovery result" });
    expect(res.textContent).toContain("Discovery succeeded");
    // The secret is never sent to the discovery test.
    expect(JSON.stringify(tested)).not.toContain("s3cret-canary");
    expect(tested!.provider.issuer).toBe("https://new.example.test");
  });

  it("saves a new provider with its secret as a sealed write, and a kept one as keep", async () => {
    let put: Json | null = null;
    await open({
      "POST /v1/admin/settings/sign_in/preview": () => json(200, preview()),
      "PUT /v1/admin/settings/sign_in": async (req) => {
        put = await bodyOf(req);
        return json(200, signIn());
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add provider…" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Provider id"), { target: { value: "new" } });
    fireEvent.change(within(dialog).getByLabelText("Issuer"), { target: { value: "https://new.example.test" } });
    fireEvent.change(within(dialog).getByLabelText("Client id"), { target: { value: "cid" } });
    fireEvent.change(within(dialog).getByLabelText("Client secret: new value"), { target: { value: "s3cret" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(put).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(put).not.toBeNull());
    const list = put!.values["auth.oidc_providers"] as Json[];
    expect(list.map((e) => e.id)).toEqual(["corp", "new"]);
    expect(list[0]!.client_secret).toEqual({ $secret: "keep" });
    expect(list[1]!.client_secret).toEqual({ $secret: "s3cret" });
    expect(list[1]!.client_secret_env).toBeUndefined();
  });

  it("asks for the secret again when the issuer or client id of a stored provider changes", async () => {
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Edit corp" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("A client secret is stored (sealed).")).toBeTruthy();
    const done = within(dialog).getByRole("button", { name: "Done" }) as HTMLButtonElement;
    expect(done.disabled).toBe(false);
    fireEvent.change(within(dialog).getByLabelText("Issuer"), { target: { value: "https://other.example.test" } });
    expect(within(dialog).getByText(/needs the client secret entered again/)).toBeTruthy();
    expect(done.disabled).toBe(true);
    fireEvent.click(within(dialog).getByRole("button", { name: "Replace…" }));
    fireEvent.change(within(dialog).getByLabelText("Client secret: new value"), { target: { value: "again" } });
    expect(done.disabled).toBe(false);
  });

  it("refuses an id that Helm already defines and never offers an environment variable or file", async () => {
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Add provider…" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Provider id"), { target: { value: "helm" } });
    expect(within(dialog).getByText(/is defined in Helm/)).toBeTruthy();
    expect(within(dialog).queryByLabelText(/environment|file/i)).toBeNull();
  });

  it("asks for confirmation naming an instance-admin grant before saving", async () => {
    let put = false;
    await open({
      "POST /v1/admin/settings/sign_in/preview": () =>
        json(200, preview({ loosening: [{ key: "auth.oidc_providers", entry_id: "corp", reason: "a mapping to instance_admin was added" }], warnings: [{ code: "grants_instance_admin", message: "Everyone with the claim value ops becomes an instance administrator." }] })),
      "PUT /v1/admin/settings/sign_in": () => {
        put = true;
        return json(200, signIn());
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Edit corp" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Instance administrator" }));
    expect(within(dialog).getByText(/gives everyone who carries that claim value full control/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const confirm = await screen.findByRole("dialog");
    expect(within(confirm).getByText(/This allows more than before/)).toBeTruthy();
    expect(within(confirm).getByText(/becomes an instance administrator/)).toBeTruthy();
    expect(put).toBe(false);
    fireEvent.click(within(confirm).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(put).toBe(true));
  });

  it("shows the server's refusal of the last provider and of a failed discovery on the entry", async () => {
    await open({
      "POST /v1/admin/settings/sign_in/preview": () =>
        json(422, { error: { code: "settings_invalid", message: "Settings are invalid.", details: { fields: [{ path: "/auth.oidc_providers/corp/issuer", code: "oidc_discovery_failed", message: "The issuer's discovery failed." }] } } }),
    });
    fireEvent.click(screen.getByRole("button", { name: "Remove corp" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("alert");
  });
});
