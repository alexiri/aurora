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
const SETUP_FIELDS = [
  { name: "server_url", label: "Server URL", type: "url", required: true, placeholder: "https://chat.example.org" },
  { name: "request_timeout_secs", label: "Request timeout (seconds)", type: "number", required: false, default: 30, min: 1, max: 120 },
  { name: "api_token", label: "API token", type: "secret", required: true },
  { name: "admin_token", label: "Admin token", type: "secret", required: false },
  { name: "signing_key", label: "Signing key", type: "generated_secret", required: false },
];
const manifest = {
  package: { id: "mattermost" },
  capabilities: { threads: true, files: true, edits: false },
  public_callbacks: ["actions", "commands"],
  integration_setup: { fields: [{ name: "bot_token", label: "Bot token", type: "secret", store: "credentials" }] },
  instance_setup: { fields: SETUP_FIELDS },
};
const mattermost = {
  type_id: "mattermost", name: "Mattermost", version: "1.2.0", publisher: "Aurora", icon_png_b64: null,
  setup_guide_md: "# Mattermost\n\nConnect a **Mattermost** team with a bot account.\n\nMore.",
  manifest, manifest_digest: "sha256:one", instance_setup: { fields: SETUP_FIELDS }, hosts: 2, added: false,
};
const types = (over: Record<string, unknown> = {}, host_running = true) => ({ host_running, items: [{ ...mattermost, ...over }] });

const managed = {
  id: "mattermost", display_name: "Mattermost", status: "active", version: "1.2.0", contract: 1, managed: true, type_id: "mattermost",
  manifest, manifest_digest: "sha256:one", approved_manifest_digest: "sha256:one", approved_manifest: manifest, widening: [],
  replicas: [], integration_count: 0, approved_at: T, created_at: T,
  settings: { server_url: "https://chat.example.org", request_timeout_secs: 30 },
  secrets: {
    api_token: { set: true, generated: false, updated_at: T },
    admin_token: { set: true, generated: false, updated_at: T },
    signing_key: { set: true, generated: true, updated_at: T },
  },
  host_status: [{ host_instance: "11111111-1111-4111-8111-111111111111", state: "running", message: null, at: T }],
};
const selfHosted = { ...managed, id: "custom", display_name: "Custom bridge", managed: false, type_id: null, settings: null, secrets: {}, host_status: [], integration_count: 1 };

const add = (calls: Rec[]) => calls.filter((c) => c.key === "POST /v1/admin/connectors");

async function openAdd(routes: Record<string, Handler>) {
  const calls = mockFetch({ "GET /v1/admin/connectors": () => json(200, { items: [] }), ...routes });
  wrap(<ConnectorsAdminView />);
  fireEvent.click(await screen.findByRole("button", { name: "Add connector" }));
  return calls;
}

describe("Add connector flow", () => {
  it("says the connector host is not running and keeps the self-hosted path", async () => {
    await openAdd({ "GET /v1/admin/connector-types": () => json(200, { host_running: false, items: [] }) });
    expect(await screen.findByText("The connector host is not running")).toBeInTheDocument();
    expect(screen.getByText(/docs\/operator\/plugins-and-connectors\.md/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Self-hosted connector (advanced)" })).toBeInTheDocument();
  });

  it("lists types with publisher, version and description, and marks added ones", async () => {
    await openAdd({ "GET /v1/admin/connector-types": () => json(200, types()) });
    const list = within(await screen.findByRole("list", { name: "Connector types" }));
    expect(list.getByText("Mattermost")).toBeInTheDocument();
    expect(list.getByText(/v1\.2\.0 · Aurora/)).toBeInTheDocument();
    expect(list.getByText("Connect a Mattermost team with a bot account.")).toBeInTheDocument();
    expect(list.getByRole("button", { name: "Add Mattermost" })).toBeInTheDocument();
    cleanup();
    await openAdd({ "GET /v1/admin/connector-types": () => json(200, types({ added: true })) });
    const added = within(await screen.findByRole("list", { name: "Connector types" }));
    expect(added.getByText("Added")).toBeInTheDocument();
    expect(added.queryByRole("button", { name: "Add Mattermost" })).toBeNull();
  });

  it("reviews the manifest, renders the settings form, maps field errors and adds in one request", async () => {
    let attempts = 0;
    const calls = await openAdd({
      "GET /v1/admin/connector-types": () => json(200, types()),
      "POST /v1/admin/connectors": () => {
        attempts += 1;
        if (attempts === 1) {
          return err(422, "invalid_request", "invalid", { fields: [{ path: "settings.server_url", message: "Must be an https URL." }] });
        }
        return json(201, { connector: managed });
      },
      "GET /v1/admin/connectors": () => json(200, { items: attempts >= 2 ? [managed] : [] }),
    });
    fireEvent.click(await screen.findByRole("button", { name: "Add Mattermost" }));
    // Review
    const summary = within(await screen.findByTestId("manifest-summary"));
    expect(summary.getByText("files")).toBeInTheDocument();
    expect(summary.queryByText("edits")).toBeNull();
    expect(summary.getByText("actions")).toBeInTheDocument();
    expect(screen.getByText("Signature verified")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    // Settings
    const url = await screen.findByLabelText("Server URL");
    const token = screen.getByLabelText("API token");
    expect(token).toHaveAttribute("type", "password");
    expect(token).toHaveAttribute("autocomplete", "off");
    expect(screen.getByLabelText("Request timeout (seconds) (optional)")).toHaveAttribute("placeholder", "30");
    expect(screen.getByTestId("generated-signing_key").textContent).toBe("Generated automatically");
    expect(screen.queryByLabelText("Signing key")).toBeNull();
    // Required fields are checked before any request.
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    expect(await screen.findByText("Server URL is required.")).toBeInTheDocument();
    expect(screen.getByText("API token is required.")).toBeInTheDocument();
    expect(add(calls)).toHaveLength(0);
    fireEvent.change(url, { target: { value: "http://chat" } });
    fireEvent.change(token, { target: { value: "tok-secret" } });
    fireEvent.change(screen.getByLabelText("Request timeout (seconds) (optional)"), { target: { value: "45" } });
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    // API field error lands on its field
    expect(await screen.findByText("Must be an https URL.")).toBeInTheDocument();
    expect(url).toHaveAttribute("aria-invalid", "true");
    fireEvent.change(url, { target: { value: "https://chat.example.org" } });
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    await waitFor(() => expect(add(calls)).toHaveLength(2));
    expect(add(calls)[1].body).toEqual({
      id: "mattermost",
      managed: true,
      approve_manifest_digest: "sha256:one",
      settings: { server_url: "https://chat.example.org", request_timeout_secs: 45 },
      secrets: { api_token: "tok-secret" },
    });
    // No token dialog for a managed connector; the detail view opens and shows no secret.
    expect(await screen.findByRole("heading", { name: /Mattermost/, level: 1 })).toBeInTheDocument();
    expect(screen.queryByTestId("secret-key")).toBeNull();
    expect(document.body.textContent).not.toContain("tok-secret");
  });

  it("reloads the type and returns to Review on a stale manifest", async () => {
    let digest = "sha256:one";
    let first = true;
    const calls = await openAdd({
      "GET /v1/admin/connector-types": () => json(200, types({ manifest_digest: digest })),
      "POST /v1/admin/connectors": () => {
        if (first) {
          first = false;
          digest = "sha256:two";
          return err(409, "stale_manifest", "changed");
        }
        return json(201, { connector: managed });
      },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Add Mattermost" }));
    fireEvent.click(await screen.findByRole("button", { name: "Continue" }));
    fireEvent.change(await screen.findByLabelText("Server URL"), { target: { value: "https://chat.example.org" } });
    fireEvent.change(screen.getByLabelText("API token"), { target: { value: "tok" } });
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    expect(await screen.findByText(/changed while you were adding it/)).toBeInTheDocument();
    expect(screen.getByTestId("manifest-summary")).toBeInTheDocument();
    await waitFor(() => expect(calls.filter((c) => c.key === "GET /v1/admin/connector-types").length).toBe(2));
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    // The form keeps what was typed.
    expect(await screen.findByLabelText("Server URL")).toHaveValue("https://chat.example.org");
    fireEvent.click(screen.getByRole("button", { name: "Add connector" }));
    await waitFor(() => expect(add(calls)).toHaveLength(2));
    expect((add(calls)[1].body as { approve_manifest_digest: string }).approve_manifest_digest).toBe("sha256:two");
  });
});

describe("Connector list", () => {
  it("shows Managed and Self-hosted badges, status, integrations and hosts", async () => {
    mockFetch({
      "GET /v1/admin/connectors": () =>
        json(200, {
          items: [
            { ...managed, status: "awaiting_registration", host_status: [] },
            selfHosted,
            { ...managed, id: "broken", display_name: "Broken", host_status: [{ host_instance: "22222222-2222-4222-8222-222222222222", state: "error", message: "bad token", at: T }] },
          ],
        }),
    });
    wrap(<ConnectorsAdminView />);
    expect(await screen.findByText("Waiting for the connector host")).toBeInTheDocument();
    expect(screen.getByText("Self-hosted")).toBeInTheDocument();
    expect(screen.getAllByText("Managed").length).toBe(2);
    expect(screen.getByText("Error: bad token")).toBeInTheDocument();
    expect(screen.getByText(/1 integration\(s\) · 0 replica\(s\)/)).toBeInTheDocument();
    expect(screen.getByText(/0 integration\(s\) · 1 host\(s\) reporting/)).toBeInTheDocument();
    // Managed connectors are managed in the detail view: no token or delete on the card.
    expect(screen.getAllByRole("button", { name: /^Manage / }).length).toBe(2);
    expect(screen.getAllByRole("button", { name: "Rotate token" }).length).toBe(1);
  });
});

describe("Managed connector detail", () => {
  async function openDetail(connector: unknown, routes: Record<string, Handler> = {}) {
    const calls = mockFetch({
      "GET /v1/admin/connectors": () => json(200, { items: [connector] }),
      "GET /v1/admin/connector-types": () => json(200, types()),
      ...routes,
    });
    wrap(<ConnectorsAdminView />);
    fireEvent.click(await screen.findByRole("button", { name: /^Manage / }));
    await screen.findByRole("heading", { level: 1 });
    await screen.findByLabelText("Server URL");
    return calls;
  }
  const patches = (calls: Rec[]) => calls.filter((c) => c.key === "PATCH /v1/admin/connectors/mattermost");

  it("shows secrets as set with dates, never their values, and host status", async () => {
    await openDetail(managed);
    expect(within(screen.getByTestId("secret-api_token")).getByText(/^Set · updated /)).toBeInTheDocument();
    expect(within(screen.getByTestId("secret-signing_key")).getByText(/^Generated automatically · updated /)).toBeInTheDocument();
    expect(screen.queryByLabelText("API token", { selector: "input" })).toBeNull();
    expect(screen.getByLabelText("Server URL")).toHaveValue("https://chat.example.org");
    expect(screen.getByLabelText("Request timeout (seconds) (optional)")).toHaveValue(30);
    const hosts = within(screen.getByRole("list", { name: "Connector hosts" }));
    expect(hosts.getByText("Running")).toBeInTheDocument();
    // Required secrets cannot be cleared; optional ones can.
    expect(screen.queryByRole("button", { name: "Clear API token" })).toBeNull();
    expect(screen.getByRole("button", { name: "Clear Admin token" })).toBeInTheDocument();
  });

  it("replaces and clears secrets and sends only what changed", async () => {
    const calls = await openDetail(managed, {
      "PATCH /v1/admin/connectors/mattermost": () => json(200, managed),
    });
    fireEvent.click(screen.getByRole("button", { name: "Replace API token" }));
    const input = screen.getByLabelText("API token");
    expect(input).toHaveAttribute("type", "password");
    expect(input).toHaveAttribute("autocomplete", "off");
    fireEvent.change(input, { target: { value: "rotated-secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Clear Admin token" }));
    expect(screen.getByText("Will be cleared when you save")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Server URL"), { target: { value: "https://chat2.example.org" } });
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
    await waitFor(() => expect(patches(calls)).toHaveLength(1));
    expect(patches(calls)[0].body).toEqual({
      settings: { server_url: "https://chat2.example.org" },
      secrets: { api_token: "rotated-secret", admin_token: null },
    });
    expect(await screen.findByText("Saved.")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("rotated-secret");
    expect(screen.queryByLabelText("API token", { selector: "input" })).toBeNull();
  });

  it("restores a default with null and maps API field errors", async () => {
    const calls = await openDetail(managed, {
      "PATCH /v1/admin/connectors/mattermost": () =>
        err(422, "invalid_request", "invalid", { fields: [{ path: "settings.request_timeout_secs", message: "Must be at most 120." }] }),
    });
    fireEvent.change(screen.getByLabelText("Request timeout (seconds) (optional)"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
    expect(await screen.findByText("Must be at most 120.")).toBeInTheDocument();
    expect(patches(calls)[0].body).toEqual({ settings: { request_timeout_secs: null } });
  });

  it("regenerates a generated secret after confirmation", async () => {
    const calls = await openDetail(managed, {
      "PATCH /v1/admin/connectors/mattermost": () => json(200, managed),
    });
    fireEvent.click(screen.getByRole("button", { name: "Regenerate Signing key" }));
    expect(patches(calls)).toHaveLength(0);
    const dialog = within(await screen.findByRole("dialog"));
    fireEvent.click(dialog.getByRole("button", { name: "Regenerate" }));
    await waitFor(() => expect(patches(calls)).toHaveLength(1));
    expect(patches(calls)[0].body).toEqual({ regenerate: ["signing_key"] });
  });

  it("disables and enables", async () => {
    let current: typeof managed = managed;
    const calls = await openDetail(managed, {
      "GET /v1/admin/connectors": () => json(200, { items: [current] }),
      "PATCH /v1/admin/connectors/mattermost": (_r, body) => {
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

  it("blocks delete while integrations use the connector", async () => {
    const calls = await openDetail({ ...managed, integration_count: 3 });
    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
    expect(screen.getByText(/3 integration\(s\) still use this connector/)).toBeInTheDocument();
    expect(calls.some((c) => c.key.startsWith("DELETE"))).toBe(false);
  });

  it("deletes an unused connector after confirmation and returns to the list", async () => {
    let gone = false;
    const calls = await openDetail(managed, {
      "GET /v1/admin/connectors": () => json(200, { items: gone ? [] : [managed] }),
      "DELETE /v1/admin/connectors/mattermost": () => {
        gone = true;
        return json(204, null);
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Delete connector" }));
    await waitFor(() => expect(calls.some((c) => c.key === "DELETE /v1/admin/connectors/mattermost")).toBe(true));
    expect(await screen.findByText("No connectors")).toBeInTheDocument();
  });

  it("shows the manifest diff and approves a pending manifest", async () => {
    const pending = { ...managed, status: "pending_approval", manifest_digest: "sha256:two", widening: ["adds capability edits"], manifest: { ...manifest, capabilities: { threads: true, files: true, edits: true } } };
    const calls = await openDetail(pending, {
      "POST /v1/admin/connectors/mattermost/approve": () => json(200, { ...pending, status: "active", widening: [] }),
    });
    expect(screen.getByText("adds capability edits")).toBeInTheDocument();
    const diff = within(screen.getByTestId("manifest-diff"));
    expect(diff.getByText("edits").closest("li")?.textContent).toContain("Added:");
    fireEvent.click(screen.getByLabelText(/I have reviewed/));
    fireEvent.click(screen.getByRole("button", { name: "Approve manifest" }));
    await waitFor(() => expect(calls.find((c) => c.key.endsWith("/approve"))?.body).toEqual({ manifest_digest: "sha256:two" }));
  });
});

// A connector's detail with a type the host no longer offers still renders from its own manifest.
describe("Detail without a running host", () => {
  it("falls back to the connector's manifest for the form", async () => {
    mockFetch({
      "GET /v1/admin/connectors": () => json(200, { items: [managed] }),
      "GET /v1/admin/connector-types": () => json(200, { host_running: false, items: [] }),
    });
    wrap(<ConnectorsAdminView />);
    fireEvent.click(await screen.findByRole("button", { name: /^Manage / }));
    expect(await screen.findByLabelText("Server URL")).toBeInTheDocument();
  });
});

describe("Self-hosted path", () => {
  it("still shows the registration token once", async () => {
    mockFetch({
      "GET /v1/admin/connectors": () => json(200, { items: [] }),
      "GET /v1/admin/connector-types": () => json(200, types()),
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
