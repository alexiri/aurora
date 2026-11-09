import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(""),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useParams: () => ({}),
  usePathname: () => "/",
}));

import { ConnectorsAdminView } from "@/components/connectors/connectors-admin-view";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const err = (status: number, code: string, message = code, details?: unknown) =>
  json(status, { error: { code, message, details } });
type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
interface Rec { key: string; body: unknown }
function mockFetch(routes: Record<string, Handler>) {
  const calls: Rec[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      const key = `${req.method} ${url.pathname}`;
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const body = text ? (JSON.parse(text) as unknown) : null;
      calls.push({ key, body });
      const h = routes[key];
      return h ? h(req, body) : err(404, "not_found");
    }),
  );
  return calls;
}
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const T = "2026-01-01T00:00:00Z";

// Mattermost (docs/13 §3.1): instance settings only; the server URL and admin token stay per integration.
const MM_FIELDS = [
  {
    name: "public_url", label: "Callback URL", type: "url", required: false, placeholder: "https://aurora.example.org/connectors/mattermost",
    help: "Where Mattermost reaches Aurora. Leave empty to use <external URL>/connectors/mattermost.",
  },
  { name: "callback_hmac_key", label: "Callback signing key", type: "generated_secret", required: false },
  { name: "catchup_window_secs", label: "Catch-up window (seconds)", type: "number", required: false, default: 600, min: 1, max: 86400 },
  { name: "token_check_secs", label: "Token check interval (seconds)", type: "number", required: false, default: 30, min: 1, max: 3600 },
];
const mmManifest = {
  package: { id: "mattermost" },
  capabilities: { threads: true, files: true, edits: false },
  public_callbacks: ["actions", "commands"],
  integration_setup: { fields: [{ name: "bot_token", label: "Bot token", type: "secret", store: "credentials" }] },
  instance_setup: { fields: MM_FIELDS },
};
const mattermost = {
  type_id: "mattermost", name: "Mattermost", version: "1.2.0", publisher: "Aurora", icon_png_b64: null,
  setup_guide_md: "# Mattermost\n\nConnect a **Mattermost** team with a bot account.\n\nMore.",
  manifest: mmManifest, manifest_digest: "sha256:mm", instance_setup: { fields: MM_FIELDS }, hosts: 2, added: false,
};
const mmConnector = {
  id: "mattermost", display_name: "Mattermost", status: "active", version: "1.2.0", contract: 1, managed: true, type_id: "mattermost",
  manifest: mmManifest, manifest_digest: "sha256:mm", approved_manifest_digest: "sha256:mm", approved_manifest: mmManifest, widening: [],
  replicas: [], integration_count: 0, approved_at: T, created_at: T,
  settings: { public_url: null, catchup_window_secs: 600, token_check_secs: 30 },
  secrets: { callback_hmac_key: { set: true, generated: true, updated_at: T } },
  host_status: [{ host_instance: "11111111-1111-4111-8111-111111111111", state: "running", message: null, at: T }],
};

// The fake type (development only) carries the one secret field the secret tests need.
const fakeFields = (secretRequired: boolean) => [
  { name: "base_url", label: "Base URL", type: "url", required: true, placeholder: "https://fake.example.org" },
  { name: "retry_limit", label: "Retry limit", type: "number", required: false, default: 3, min: 1, max: 10 },
  { name: "shared_secret", label: "Shared secret", type: "secret", required: secretRequired },
  { name: "signing_key", label: "Signing key", type: "generated_secret", required: false },
];
const fakeManifest = (secretRequired = true) => ({
  package: { id: "fake" },
  capabilities: { threads: true, files: true, edits: false },
  public_callbacks: ["hook"],
  integration_setup: { fields: [] },
  instance_setup: { fields: fakeFields(secretRequired) },
});
const fakeType = (over: Record<string, unknown> = {}, secretRequired = true) => ({
  type_id: "fake", name: "Fake (development)", version: "0.1.0", publisher: "Aurora", icon_png_b64: null,
  setup_guide_md: "A fake chat platform for development.", manifest: fakeManifest(secretRequired), manifest_digest: "sha256:one",
  instance_setup: { fields: fakeFields(secretRequired) }, hosts: 1, added: false, ...over,
});
const fakeConnector = (secretRequired = true) => ({
  ...mmConnector, id: "fake", display_name: "Fake (development)", type_id: "fake", version: "0.1.0",
  manifest: fakeManifest(secretRequired), manifest_digest: "sha256:one", approved_manifest_digest: "sha256:one", approved_manifest: fakeManifest(secretRequired),
  settings: { base_url: "https://fake.example.org", retry_limit: 3 },
  secrets: {
    shared_secret: { set: true, generated: false, updated_at: T },
    signing_key: { set: true, generated: true, updated_at: T },
  },
});
const types = (items: unknown[], host_running = true) => ({ host_running, items });
const selfHosted = { ...mmConnector, id: "custom", display_name: "Custom bridge", managed: false, type_id: null, settings: null, secrets: {}, host_status: [], integration_count: 1 };

const add = (calls: Rec[]) => calls.filter((c) => c.key === "POST /v1/admin/connectors");

async function openAdd(routes: Record<string, Handler>) {
  const calls = mockFetch({ "GET /v1/admin/connectors": () => json(200, { items: [] }), ...routes });
  wrap(<ConnectorsAdminView />);
  fireEvent.click(await screen.findByRole("button", { name: "Add connector" }));
  return calls;
}

describe("Add connector flow", () => {
  it("says the connector host is not running and keeps the self-hosted path", async () => {
    await openAdd({ "GET /v1/admin/connector-types": () => json(200, types([], false)) });
    expect(await screen.findByText("The connector host is not running")).toBeInTheDocument();
    expect(screen.getByText(/docs\/operator\/plugins-and-connectors\.md/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Self-hosted connector (advanced)" })).toBeInTheDocument();
  });

  it("lists types with publisher, version and description, and marks added ones", async () => {
    await openAdd({ "GET /v1/admin/connector-types": () => json(200, types([mattermost])) });
    const list = within(await screen.findByRole("list", { name: "Connector types" }));
    expect(list.getByText("Mattermost")).toBeInTheDocument();
    expect(list.getByText(/v1\.2\.0 · Aurora/)).toBeInTheDocument();
    expect(list.getByText("Connect a Mattermost team with a bot account.")).toBeInTheDocument();
    expect(list.getByRole("button", { name: "Add Mattermost" })).toBeInTheDocument();
    cleanup();
    await openAdd({ "GET /v1/admin/connector-types": () => json(200, types([{ ...mattermost, added: true }])) });
    const added = within(await screen.findByRole("list", { name: "Connector types" }));
    expect(added.getByText("Added")).toBeInTheDocument();
    expect(added.queryByRole("button", { name: "Add Mattermost" })).toBeNull();
  });

  it("renders Mattermost's instance settings: help text once, placeholders, generated key, defaults", async () => {
    const calls = await openAdd({
      "GET /v1/admin/connector-types": () => json(200, types([mattermost])),
      "POST /v1/admin/connectors": () => json(201, { connector: mmConnector }),
      "GET /v1/admin/connectors": () => json(200, { items: [] }),
    });
    fireEvent.click(await screen.findByRole("button", { name: "Add Mattermost" }));
    fireEvent.click(await screen.findByRole("button", { name: "Continue" }));
    const url = await screen.findByLabelText("Callback URL (optional)");
    expect(url).toHaveAttribute("placeholder", "https://aurora.example.org/connectors/mattermost");
    // The placeholder is not repeated as help text; the help is its own sentence.
    expect(screen.queryByText("https://aurora.example.org/connectors/mattermost")).toBeNull();
    expect(screen.getByText("Where Mattermost reaches Aurora. Leave empty to use <external URL>/connectors/mattermost.")).toBeInTheDocument();
    expect(screen.getByTestId("generated-callback_hmac_key").textContent).toBe("Generated automatically");
    expect(screen.queryByLabelText("Callback signing key (optional)")).toBeNull();
    expect(screen.getByLabelText("Catch-up window (seconds) (optional)")).toHaveAttribute("placeholder", "600");
    expect(screen.getByLabelText("Token check interval (seconds) (optional)")).toHaveAttribute("placeholder", "30");
    // No server URL or admin token at instance level.
    expect(screen.queryByLabelText(/Server URL/)).toBeNull();
    fireEvent.change(screen.getByLabelText("Catch-up window (seconds) (optional)"), { target: { value: "900" } });
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    await waitFor(() => expect(add(calls)).toHaveLength(1));
    expect(add(calls)[0].body).toEqual({
      id: "mattermost", managed: true, approve_manifest_digest: "sha256:mm", settings: { catchup_window_secs: 900 }, secrets: {},
    });
  });

  it("reviews the manifest, renders secret and generated fields, maps field errors and adds in one request", async () => {
    let attempts = 0;
    const calls = await openAdd({
      "GET /v1/admin/connector-types": () => json(200, types([fakeType()])),
      "POST /v1/admin/connectors": () => {
        attempts += 1;
        if (attempts === 1) {
          return err(422, "invalid_request", "invalid", { fields: [{ path: "settings.base_url", message: "Must be an https URL." }] });
        }
        return json(201, { connector: fakeConnector() });
      },
      "GET /v1/admin/connectors": () => json(200, { items: attempts >= 2 ? [fakeConnector()] : [] }),
    });
    fireEvent.click(await screen.findByRole("button", { name: "Add Fake (development)" }));
    // Review
    const summary = within(await screen.findByTestId("manifest-summary"));
    expect(summary.getByText("files")).toBeInTheDocument();
    expect(summary.queryByText("edits")).toBeNull();
    expect(summary.getByText("hook")).toBeInTheDocument();
    expect(screen.getByText("Signature verified")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    // Settings
    const url = await screen.findByLabelText("Base URL");
    const secret = screen.getByLabelText("Shared secret");
    expect(secret).toHaveAttribute("type", "password");
    expect(secret).toHaveAttribute("autocomplete", "off");
    expect(screen.getByLabelText("Retry limit (optional)")).toHaveAttribute("placeholder", "3");
    expect(screen.getByTestId("generated-signing_key").textContent).toBe("Generated automatically");
    expect(screen.queryByLabelText("Signing key")).toBeNull();
    // Required fields are checked before any request.
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    expect(await screen.findByText("Base URL is required.")).toBeInTheDocument();
    expect(screen.getByText("Shared secret is required.")).toBeInTheDocument();
    expect(add(calls)).toHaveLength(0);
    fireEvent.change(url, { target: { value: "http://fake" } });
    fireEvent.change(secret, { target: { value: "tok-secret" } });
    fireEvent.change(screen.getByLabelText("Retry limit (optional)"), { target: { value: "5" } });
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    // API field error lands on its field
    expect(await screen.findByText("Must be an https URL.")).toBeInTheDocument();
    expect(url).toHaveAttribute("aria-invalid", "true");
    fireEvent.change(url, { target: { value: "https://fake.example.org" } });
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    await waitFor(() => expect(add(calls)).toHaveLength(2));
    expect(add(calls)[1].body).toEqual({
      id: "fake",
      managed: true,
      approve_manifest_digest: "sha256:one",
      settings: { base_url: "https://fake.example.org", retry_limit: 5 },
      secrets: { shared_secret: "tok-secret" },
    });
    // No token dialog for a managed connector; the detail view opens and shows no secret.
    expect(await screen.findByRole("heading", { name: /Fake \(development\)/, level: 1 })).toBeInTheDocument();
    expect(screen.queryByTestId("secret-key")).toBeNull();
    expect(document.body.textContent).not.toContain("tok-secret");
  });

  it("reloads the type and returns to Review on a stale manifest", async () => {
    let digest = "sha256:one";
    let first = true;
    const calls = await openAdd({
      "GET /v1/admin/connector-types": () => json(200, types([fakeType({ manifest_digest: digest })])),
      "POST /v1/admin/connectors": () => {
        if (first) {
          first = false;
          digest = "sha256:two";
          return err(409, "stale_manifest", "changed");
        }
        return json(201, { connector: fakeConnector() });
      },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Add Fake (development)" }));
    fireEvent.click(await screen.findByRole("button", { name: "Continue" }));
    fireEvent.change(await screen.findByLabelText("Base URL"), { target: { value: "https://fake.example.org" } });
    fireEvent.change(screen.getByLabelText("Shared secret"), { target: { value: "tok" } });
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    expect(await screen.findByText(/changed while you were adding it/)).toBeInTheDocument();
    expect(screen.getByTestId("manifest-summary")).toBeInTheDocument();
    await waitFor(() => expect(calls.filter((c) => c.key === "GET /v1/admin/connector-types").length).toBe(2));
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    // The form keeps what was typed.
    expect(await screen.findByLabelText("Base URL")).toHaveValue("https://fake.example.org");
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    await waitFor(() => expect(add(calls)).toHaveLength(2));
    expect((add(calls)[1].body as { approve_manifest_digest: string }).approve_manifest_digest).toBe("sha256:two");
  });
});

describe("Connector list", () => {
  it("shows Managed and Self-hosted badges, status, singular/plural counts and hosts", async () => {
    mockFetch({
      "GET /v1/admin/connectors": () =>
        json(200, {
          items: [
            { ...mmConnector, status: "awaiting_registration", host_status: [], integration_count: 2 },
            selfHosted,
            { ...mmConnector, id: "broken", display_name: "Broken", integration_count: 0, host_status: [{ host_instance: "22222222-2222-4222-8222-222222222222", state: "error", message: "bad token", at: T }] },
          ],
        }),
    });
    wrap(<ConnectorsAdminView />);
    expect(await screen.findByText("Waiting for the connector host")).toBeInTheDocument();
    expect(screen.getByText("Self-hosted")).toBeInTheDocument();
    expect(screen.getAllByText("Managed").length).toBe(2);
    expect(screen.getByText("Error: bad token")).toBeInTheDocument();
    expect(screen.getByText(/^1 integration · 0 replicas$/)).toBeInTheDocument();
    expect(screen.getByText(/^2 integrations · 0 hosts reporting$/)).toBeInTheDocument();
    expect(screen.getByText(/^0 integrations · 1 host reporting$/)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("(s)");
    // Managed connectors are managed in the detail view: no token or delete on the card.
    expect(screen.getAllByRole("button", { name: /^Manage / }).length).toBe(2);
    expect(screen.getAllByRole("button", { name: "Rotate token" }).length).toBe(1);
  });
});

describe("Managed connector detail", () => {
  async function openDetail(connector: unknown, fieldsType: unknown, routes: Record<string, Handler> = {}, label = "Base URL") {
    const calls = mockFetch({
      "GET /v1/admin/connectors": () => json(200, { items: [connector] }),
      "GET /v1/admin/connector-types": () => json(200, types([fieldsType])),
      ...routes,
    });
    wrap(<ConnectorsAdminView />);
    fireEvent.click(await screen.findByRole("button", { name: /^Manage / }));
    await screen.findByRole("heading", { level: 1 });
    await screen.findByLabelText(label);
    return calls;
  }
  const patches = (calls: Rec[]) => calls.filter((c) => c.key === "PATCH /v1/admin/connectors/fake");
  const optional = () => openDetail(fakeConnector(false), fakeType({}, false), { "PATCH /v1/admin/connectors/fake": () => json(200, fakeConnector(false)) });

  it("shows secrets as set with dates, never their values, and host status", async () => {
    await openDetail(fakeConnector(), fakeType());
    expect(within(screen.getByTestId("secret-shared_secret")).getByText(/^Set · updated /)).toBeInTheDocument();
    expect(within(screen.getByTestId("secret-signing_key")).getByText(/^Generated automatically · updated /)).toBeInTheDocument();
    expect(screen.queryByLabelText("Shared secret", { selector: "input" })).toBeNull();
    expect(screen.getByLabelText("Base URL")).toHaveValue("https://fake.example.org");
    expect(screen.getByLabelText("Retry limit (optional)")).toHaveValue(3);
    const hosts = within(screen.getByRole("list", { name: "Connector hosts" }));
    expect(hosts.getByText("Running")).toBeInTheDocument();
    // A required secret cannot be cleared.
    expect(screen.queryByRole("button", { name: "Clear Shared secret" })).toBeNull();
    expect(screen.getByRole("button", { name: "Replace Shared secret" })).toBeInTheDocument();
  });

  it("offers Clear for an optional secret", async () => {
    await optional();
    expect(screen.getByRole("button", { name: "Clear Shared secret" })).toBeInTheDocument();
  });

  it("replaces a secret and sends only what changed", async () => {
    const calls = await openDetail(fakeConnector(), fakeType(), { "PATCH /v1/admin/connectors/fake": () => json(200, fakeConnector()) });
    fireEvent.click(screen.getByRole("button", { name: "Replace Shared secret" }));
    const input = screen.getByLabelText("Shared secret");
    expect(input).toHaveAttribute("type", "password");
    expect(input).toHaveAttribute("autocomplete", "off");
    fireEvent.change(input, { target: { value: "rotated-secret" } });
    fireEvent.change(screen.getByLabelText("Base URL"), { target: { value: "https://fake2.example.org" } });
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
    await waitFor(() => expect(patches(calls)).toHaveLength(1));
    expect(patches(calls)[0].body).toEqual({
      settings: { base_url: "https://fake2.example.org" },
      secrets: { shared_secret: "rotated-secret" },
    });
    expect(await screen.findByText("Saved.")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("rotated-secret");
    expect(screen.queryByLabelText("Shared secret", { selector: "input" })).toBeNull();
  });

  it("clears an optional secret when saved", async () => {
    const calls = await optional();
    fireEvent.click(screen.getByRole("button", { name: /^Clear Shared secret/ }));
    expect(screen.getByText("Will be cleared when you save")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
    await waitFor(() => expect(patches(calls)).toHaveLength(1));
    expect(patches(calls)[0].body).toEqual({ secrets: { shared_secret: null } });
  });

  it("restores a default with null and maps API field errors", async () => {
    const calls = await openDetail(fakeConnector(), fakeType(), {
      "PATCH /v1/admin/connectors/fake": () =>
        err(422, "invalid_request", "invalid", { fields: [{ path: "settings.retry_limit", message: "Must be at most 10." }] }),
    });
    fireEvent.change(screen.getByLabelText("Retry limit (optional)"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
    expect(await screen.findByText("Must be at most 10.")).toBeInTheDocument();
    expect(patches(calls)[0].body).toEqual({ settings: { retry_limit: null } });
  });

  it("regenerates a generated secret after confirmation", async () => {
    const calls = await openDetail(fakeConnector(), fakeType(), { "PATCH /v1/admin/connectors/fake": () => json(200, fakeConnector()) });
    fireEvent.click(screen.getByRole("button", { name: "Regenerate Signing key" }));
    expect(patches(calls)).toHaveLength(0);
    const dialog = within(await screen.findByRole("dialog"));
    fireEvent.click(dialog.getByRole("button", { name: "Regenerate" }));
    await waitFor(() => expect(patches(calls)).toHaveLength(1));
    expect(patches(calls)[0].body).toEqual({ regenerate: ["signing_key"] });
  });

  it("disables and enables", async () => {
    let current: ReturnType<typeof fakeConnector> = fakeConnector();
    const calls = await openDetail(current, fakeType(), {
      "GET /v1/admin/connectors": () => json(200, { items: [current] }),
      "PATCH /v1/admin/connectors/fake": (_r, body) => {
        current = { ...current, status: (body as { enabled: boolean }).enabled ? "active" : "disabled" };
        return json(200, current);
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Disable" }));
    expect(await screen.findByRole("button", { name: "Enable" })).toBeInTheDocument();
    expect(patches(calls)[0].body).toEqual({ enabled: false });
    expect(screen.getAllByText("Disabled").length).toBeGreaterThan(0);
    fireEvent.click(screen.getByRole("button", { name: "Enable" }));
    expect(await screen.findByRole("button", { name: "Disable" })).toBeInTheDocument();
    expect(patches(calls)[1].body).toEqual({ enabled: true });
  });

  it("blocks delete while integrations use the connector, with correct singular/plural", async () => {
    const calls = await openDetail({ ...fakeConnector(), integration_count: 3 }, fakeType());
    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
    expect(screen.getByText("3 integrations still use this connector. Remove them first.")).toBeInTheDocument();
    expect(screen.getByText(/^3 integrations · v0\.1\.0$/)).toBeInTheDocument();
    expect(calls.some((c) => c.key.startsWith("DELETE"))).toBe(false);
    cleanup();
    await openDetail({ ...fakeConnector(), integration_count: 1 }, fakeType());
    expect(screen.getByText("1 integration still uses this connector. Remove it first.")).toBeInTheDocument();
  });

  it("deletes an unused connector after confirmation and returns to the list", async () => {
    let gone = false;
    const calls = await openDetail(fakeConnector(), fakeType(), {
      "GET /v1/admin/connectors": () => json(200, { items: gone ? [] : [fakeConnector()] }),
      "DELETE /v1/admin/connectors/fake": () => {
        gone = true;
        return json(204, null);
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Delete connector" }));
    await waitFor(() => expect(calls.some((c) => c.key === "DELETE /v1/admin/connectors/fake")).toBe(true));
    expect(await screen.findByText("No connectors")).toBeInTheDocument();
  });

  it("shows the manifest diff and approves a pending manifest", async () => {
    const base = fakeConnector();
    const pending = { ...base, status: "pending_approval", manifest_digest: "sha256:two", widening: ["adds capability edits"], manifest: { ...fakeManifest(), capabilities: { threads: true, files: true, edits: true } } };
    const calls = await openDetail(pending, fakeType(), {
      "POST /v1/admin/connectors/fake/approve": () => json(200, { ...pending, status: "active", widening: [] }),
    });
    expect(screen.getByText("adds capability edits")).toBeInTheDocument();
    const diff = within(screen.getByTestId("manifest-diff"));
    expect(diff.getByText("edits").closest("li")?.textContent).toContain("Added:");
    fireEvent.click(screen.getByLabelText(/I have reviewed/));
    fireEvent.click(screen.getByRole("button", { name: "Approve manifest" }));
    await waitFor(() => expect(calls.find((c) => c.key.endsWith("/approve"))?.body).toEqual({ manifest_digest: "sha256:two" }));
  });

  it("shows Mattermost's own instance settings in its detail view", async () => {
    await openDetail(mmConnector, mattermost, {}, "Callback URL (optional)");
    expect(screen.getByLabelText("Catch-up window (seconds) (optional)")).toHaveValue(600);
    expect(screen.getByLabelText("Token check interval (seconds) (optional)")).toHaveValue(30);
    expect(within(screen.getByTestId("secret-callback_hmac_key")).getByText(/^Generated automatically · updated /)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Regenerate Callback signing key" })).toBeInTheDocument();
  });
});

// A connector's detail with a type the host no longer offers still renders from its own manifest.
describe("Detail without a running host", () => {
  it("falls back to the connector's manifest for the form", async () => {
    mockFetch({
      "GET /v1/admin/connectors": () => json(200, { items: [fakeConnector()] }),
      "GET /v1/admin/connector-types": () => json(200, types([], false)),
    });
    wrap(<ConnectorsAdminView />);
    fireEvent.click(await screen.findByRole("button", { name: /^Manage / }));
    expect(await screen.findByLabelText("Base URL")).toBeInTheDocument();
  });
});

describe("Self-hosted path", () => {
  it("still shows the registration token once", async () => {
    mockFetch({
      "GET /v1/admin/connectors": () => json(200, { items: [] }),
      "GET /v1/admin/connector-types": () => json(200, types([mattermost])),
      "POST /v1/admin/connectors": () => json(201, { connector: { ...selfHosted, status: "awaiting_registration" }, token: "aun_once" }),
    });
    wrap(<ConnectorsAdminView />);
    fireEvent.click(await screen.findByRole("button", { name: "Add connector" }));
    fireEvent.click(await screen.findByRole("button", { name: "Self-hosted connector (advanced)" }));
    fireEvent.change(await screen.findByLabelText("Connector id"), { target: { value: "custom" } });
    fireEvent.change(screen.getByLabelText("Display name"), { target: { value: "Custom bridge" } });
    fireEvent.click(screen.getByRole("button", { name: "Provision" }));
    expect((await screen.findByTestId("secret-key")).textContent).toBe("aun_once");
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    await waitFor(() => expect(screen.queryByText("aun_once")).toBeNull());
    expect(document.body.textContent).not.toContain("aun_once");
  });
});
