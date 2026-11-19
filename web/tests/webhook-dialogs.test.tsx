import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// P5-WEBHOOK-4 (C-913): the connect steps, Edit and Send test against a mocked API (P5-WEBHOOK-2 is not merged). CodeMirror is
// replaced by a textarea: the editor's own behaviour is the Files viewer's.
vi.mock("@/components/ui/code-field", () => ({
  CodeField: ({ label, initial, docKey, onChange, error }: { label: string; initial: string; docKey: string | number; onChange: (t: string) => void; error?: string }) => (
    <div>
      <textarea key={docKey} aria-label={label} defaultValue={initial} onChange={(e) => onChange(e.target.value)} />
      {error ? <p role="alert">{error}</p> : null}
    </div>
  ),
}));
vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(""),
  usePathname: () => "/o/acme/w/main/apps",
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
}));

import { SendTestDialog } from "@/components/webhook/send-test-dialog";
import { WebhookEditDialog } from "@/components/webhook/webhook-edit-dialog";
import { WebhookSignInStep } from "@/components/webhook/webhook-connect-step";
import type { AppDetail } from "@/lib/api/apps";
import type { WebhookConfig, WebhookPreset, WebhookView } from "@/lib/api/webhook";
import { emptyConfig } from "@/lib/webhook/logic";

const URL_CANARY = "https://hooks.slack.com/services/T0CANARY/B0CANARY/xyzCANARYxyz";
const SECRET_CANARY = "secret-canary-value-7731";
const SIGNING = "whsec_signing_canary_5521";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const err = (status: number, code: string, details?: unknown) => json(status, { error: { code, message: code, details } });

type Call = { method: string; path: string; body: Record<string, unknown> | null };
let calls: Call[] = [];
let routes: Record<string, (body: Record<string, unknown> | null) => Response> = {};

beforeEach(() => {
  calls = [];
  routes = {};
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | Request, init?: RequestInit) => {
      const req = typeof input === "string" ? null : input;
      const path = new URL(typeof input === "string" ? input : input.url, "http://localhost").pathname;
      const method = init?.method ?? req?.method ?? "GET";
      const raw = init?.body ?? null;
      const body = typeof raw === "string" ? (JSON.parse(raw) as Record<string, unknown>) : null;
      calls.push({ method, path, body });
      const h = routes[`${method} ${path}`];
      return h ? h(body) : err(404, "not_found");
    }),
  );
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

const slackConfig: WebhookConfig = {
  ...emptyConfig(),
  template: '{"text": {{text}}}',
  slots: [{ name: "text", type: "string", required: true, description: "The message", example: "Deploy 42 failed" }],
  preset: "slack",
};
const presets: WebhookPreset[] = [
  { id: "slack", name: "Slack", suggest_for_hosts: ["hooks.slack.com"], secret_value_names: [], config: slackConfig },
  { id: "ntfy-json", name: "ntfy", secret_value_names: ["topic"], config: { ...slackConfig, template: '{"topic": "{{secret:topic}}", "message": {{text}}}', preset: "ntfy-json" } },
  { id: "generic-json", name: "Generic JSON", secret_value_names: [], config: { ...slackConfig, template: "{{payload}}", slots: [{ name: "payload", type: "json", required: true, description: "" }], preset: "generic-json" } },
];

const app = {
  app_id: "webhook",
  name: "Outgoing webhook",
  connect: [{ id: "url", kind: "fields", fields: [], access_levels: [], scopes: [], default_scopes: [] }],
  tools: [{ id: "send", name: "Send", group: "write", description: "" }],
} as unknown as AppDetail;

function connectStep(onResult = vi.fn()) {
  routes["GET /v1/apps/webhook/webhook-presets"] = () => json(200, presets);
  wrap(<WebhookSignInStep app={app} workspaceId="ws1" orgId="org1" level="read" agentIds={["a1"]} hasWriteTools onResult={onResult} />);
  return onResult;
}

const connection = { id: "c1", scope: "workspace", app_id: "webhook", scopes: [], missing_scopes: [], label: "alerts", status: "active" };
const connected = (extra: Record<string, unknown> = {}) => json(200, { kind: "connected", connection, grants: [{ agent_id: "a1" }], missing_scopes: [], ...extra });

async function stepOne(url = URL_CANARY) {
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "alerts" } });
  fireEvent.change(screen.getByLabelText("Webhook address (URL)"), { target: { value: url } });
  fireEvent.click(screen.getByRole("button", { name: "Next" }));
  await screen.findByText(/Step 2 of 3/);
}

describe("connect steps", () => {
  it("step 1 asks for a name and an https address, and keeps the address a password field", () => {
    connectStep();
    expect(screen.getByText(/Step 1 of 3/)).toBeInTheDocument();
    expect(screen.getByLabelText("Webhook address (URL)")).toHaveAttribute("type", "password");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("Enter a name.")).toBeInTheDocument();
    expect(screen.getByText("Paste the webhook address.")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "alerts" } });
    fireEvent.change(screen.getByLabelText("Webhook address (URL)"), { target: { value: "http://example.org/x" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("The address must start with https://.")).toBeInTheDocument();
  });

  it("suggests the preset for the address's host, only as a suggestion, and the preset fills the template and slots", async () => {
    connectStep();
    await stepOne();
    expect(await screen.findByText("This address looks like Slack.")).toBeInTheDocument();
    // Nothing is chosen until the person says so.
    expect((screen.getByLabelText(/^Preset/) as HTMLSelectElement).value).toBe("");
    expect(screen.getByRole("option", { name: "Slack (suggested)" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Use the Slack preset" }));
    expect((screen.getByLabelText("Template") as HTMLTextAreaElement).value).toBe('{"text": {{text}}}');
    expect(screen.getByLabelText("Name", { selector: "#slot-0-name" })).toHaveValue("text");
  });

  it("a host with no preset suggests nothing", async () => {
    connectStep();
    await stepOne("https://example.org/hook/abc");
    await screen.findByLabelText(/^Preset/);
    expect(screen.queryByText(/This address looks like/)).toBeNull();
  });

  it("connects with a preset: the request carries the configuration, the secret values and the signing choice", async () => {
    routes["POST /v1/apps/webhook/connect"] = () => connected();
    const onResult = connectStep();
    await stepOne();
    fireEvent.change(await screen.findByLabelText(/^Preset/), { target: { value: "ntfy-json" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText(/Step 3 of 3/);
    // The preset's secret value name is asked for in the extras.
    expect(screen.getByLabelText("Name")).toHaveValue("topic");
    fireEvent.change(screen.getByLabelText("Value"), { target: { value: SECRET_CANARY } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(onResult).toHaveBeenCalled());
    const post = calls.find((c) => c.method === "POST" && c.path === "/v1/apps/webhook/connect")!;
    const body = post.body as { fields: Record<string, string>; webhook: WebhookConfig & { secret_values: Record<string, string> } };
    expect(body.fields).toEqual({ name: "alerts", url: URL_CANARY });
    expect(body.webhook.preset).toBe("ntfy-json");
    expect(body.webhook.template).toContain("{{secret:topic}}");
    expect(body.webhook.secret_values).toEqual({ topic: SECRET_CANARY });
    expect(onResult.mock.calls[0]![0]).toMatchObject({ accountId: "c1", granted: ["a1"] });
  });

  it("canary: after Save the address and the secret values are nowhere in the DOM, and no preview request sent them", async () => {
    routes["POST /v1/apps/webhook/connect"] = () => connected();
    routes["POST /v1/apps/webhook/webhook-preview"] = () => json(200, { method: "POST", content_type: "application/json", headers: [], body: '{"topic":"«secret:topic»"}' });
    const onResult = connectStep();
    await stepOne();
    fireEvent.change(await screen.findByLabelText(/^Preset/), { target: { value: "ntfy-json" } });
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    await screen.findByTestId("preview-body");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText(/Step 3 of 3/);
    fireEvent.change(screen.getByLabelText("Value"), { target: { value: SECRET_CANARY } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(onResult).toHaveBeenCalled());
    // The preview sent names, never values; the address was never in it.
    const preview = calls.find((c) => c.path.endsWith("webhook-preview"))!;
    expect(JSON.stringify(preview.body)).not.toContain(URL_CANARY);
    expect(JSON.stringify(preview.body)).not.toContain(SECRET_CANARY);
    expect((preview.body as { config: { secret_value_names: string[] } }).config.secret_value_names).toEqual(["topic"]);
    const html = document.body.innerHTML;
    expect(html).not.toContain("CANARY");
    expect(html).not.toContain(SECRET_CANARY);
  });

  it("a generated signing secret is shown once with a notice, and gone after the person confirms", async () => {
    routes["POST /v1/apps/webhook/connect"] = () => connected({ signing_secret: SIGNING });
    const onResult = connectStep();
    await stepOne();
    fireEvent.change(await screen.findByLabelText(/^Preset/), { target: { value: "generic-json" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText(/Step 3 of 3/);
    fireEvent.click(screen.getByLabelText("Make one"));
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    const once = await screen.findByTestId("signing-secret-once");
    expect(within(once).getByText("This will not be shown again.")).toBeInTheDocument();
    expect(within(once).getByText(SIGNING)).toBeInTheDocument();
    expect(onResult).not.toHaveBeenCalled();
    const post = calls.find((c) => c.path === "/v1/apps/webhook/connect")!;
    expect((post.body as { webhook: { signing_secret: unknown; signing: boolean } }).webhook.signing_secret).toEqual({ mode: "generate" });
    fireEvent.click(within(once).getByRole("button", { name: "I have saved it" }));
    expect(onResult).toHaveBeenCalledTimes(1);
    cleanup();
    expect(document.body.innerHTML).not.toContain(SIGNING);
  });

  it("maps a webhook_config_invalid path onto the template and goes back to that step", async () => {
    routes["POST /v1/apps/webhook/connect"] = () => err(422, "webhook_config_invalid", { fields: [{ path: "/template", code: "template_json" }] });
    connectStep();
    await stepOne();
    fireEvent.change(await screen.findByLabelText(/^Preset/), { target: { value: "slack" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText(/Step 3 of 3/);
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await screen.findByText(/Step 2 of 3/);
    expect(await screen.findByText(/not valid JSON once the blanks are filled in/)).toBeInTheDocument();
  });

  it("maps a header path onto the Extras step, and a rate refusal onto its field", async () => {
    routes["POST /v1/apps/webhook/connect"] = () =>
      err(422, "webhook_config_invalid", { fields: [{ path: "/headers/0/name", code: "header_name_denied" }, { path: "/rate/per_minute", code: "rate_above_ceiling" }] });
    connectStep();
    await stepOne();
    fireEvent.change(await screen.findByLabelText(/^Preset/), { target: { value: "slack" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText(/Step 3 of 3/);
    fireEvent.click(screen.getByRole("button", { name: "Add a header" }));
    fireEvent.change(screen.getByLabelText("Header name"), { target: { value: "Host" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByText("Aurora sets this header itself. Choose another name.")).toBeInTheDocument();
    expect(screen.getByText("This is above the limit your organization or administrator allows.")).toBeInTheDocument();
  });

  it("maps link_not_https and a refused destination onto the address field on step 1", async () => {
    routes["POST /v1/apps/webhook/connect"] = () => err(422, "link_destination_refused");
    connectStep();
    await stepOne();
    fireEvent.change(await screen.findByLabelText(/^Preset/), { target: { value: "slack" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText(/Step 3 of 3/);
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await screen.findByText(/Step 1 of 3/);
    expect(screen.getByText(/Aurora may not send to that address/)).toBeInTheDocument();
  });

  it("a preview refusal marks the field in the editor and a bad sample value marks that value", async () => {
    routes["POST /v1/apps/webhook/webhook-preview"] = (b) => {
      const v = (b as { values: Record<string, unknown> }).values;
      return v.text === "" ? err(422, "invalid_slot", { slot: "text", rule: "missing" }) : err(422, "webhook_config_invalid", { fields: [{ path: "/slots/0/name", code: "slot_name" }] });
    };
    connectStep();
    await stepOne();
    fireEvent.change(await screen.findByLabelText(/^Preset/), { target: { value: "slack" } });
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText(/Use a lowercase name of up to 40 letters/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("text", { selector: "#pv-0" }), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    // An empty required string is sent for the server to name.
    expect(await screen.findByText("This value is required.")).toBeInTheDocument();
  });

  it("starts from an example: a Slack-style body becomes {{text}} and a string slot, and choosing it again makes it fixed", async () => {
    connectStep();
    await stepOne();
    await screen.findByLabelText(/^Preset/);
    fireEvent.click(screen.getByText("Start from an example"));
    fireEvent.change(screen.getByLabelText("The example body"), { target: { value: '{"username": "Aurora", "text": "Hello"}' } });
    fireEvent.click(screen.getByRole("button", { name: "Show the example" }));
    const pick = await screen.findByRole("button", { name: "Agent fills in text" });
    expect(pick).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(pick);
    expect(screen.getByRole("button", { name: "Make fixed text" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByTestId("example-template").textContent).toContain('"text": {{text}}');
    expect(screen.getByTestId("example-template").textContent).toContain('"username": "Aurora"');
    fireEvent.click(screen.getByRole("button", { name: "Use this template" }));
    expect((screen.getByLabelText("Template") as HTMLTextAreaElement).value).toContain('"text": {{text}}');
    expect(screen.getByLabelText("Name", { selector: "#slot-0-name" })).toHaveValue("text");
    expect(screen.getByLabelText("Kind of value")).toHaveValue("string");
    // Un-making it restores the constant.
    fireEvent.click(screen.getByRole("button", { name: "Make fixed text" }));
    expect(screen.getByTestId("example-template").textContent).toContain('"text": "Hello"');
  });

  it("refuses an example that is not JSON, in words", async () => {
    connectStep();
    await stepOne();
    await screen.findByLabelText(/^Preset/);
    fireEvent.click(screen.getByText("Start from an example"));
    fireEvent.change(screen.getByLabelText("The example body"), { target: { value: "{nope" } });
    fireEvent.click(screen.getByRole("button", { name: "Show the example" }));
    expect(await screen.findByText(/That is not valid JSON/)).toBeInTheDocument();
  });
});

const view = (over: Partial<WebhookView> = {}): WebhookView => ({
  ...slackConfig,
  signing: true,
  host: "hooks.slack.com",
  secret_value_names: ["topic"],
  effective_rate: { per_minute: 30, per_day: 500, source: "org" },
  ...over,
});

describe("Edit", () => {
  it("shows the secret values only by name, with Replace and Delete, and never an address", async () => {
    routes["GET /v1/connections/c1/webhook"] = () => json(200, view({ template: '{"text": {{text}}, "t": "{{secret:topic}}"}' }));
    routes["GET /v1/apps/webhook/webhook-presets"] = () => json(200, presets);
    routes["PUT /v1/connections/c1/webhook"] = () => json(200, view());
    const onOpenChange = vi.fn();
    wrap(<WebhookEditDialog connectionId="c1" name="alerts" open onOpenChange={onOpenChange} />);
    await screen.findByLabelText("Template");
    expect(screen.getByText("Sends to", { exact: false }).textContent).toContain("hooks.slack.com");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText("Limits");
    expect(screen.getByText("topic")).toBeInTheDocument();
    expect(screen.queryByLabelText("Value")).toBeNull();
    expect(screen.getByTestId("wh-effective").textContent).toContain("30 a minute and 500 a day, from your organization's limit");
    fireEvent.click(screen.getByRole("button", { name: "Replace topic" }));
    fireEvent.change(screen.getByLabelText("New value of topic"), { target: { value: SECRET_CANARY } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    const put = calls.find((c) => c.method === "PUT")!;
    expect((put.body as { secret_values: unknown }).secret_values).toEqual({ topic: SECRET_CANARY });
    // Signing is kept unless the person changed it.
    expect((put.body as { signing_secret: unknown }).signing_secret).toEqual({ mode: "keep" });
    expect(document.body.innerHTML).not.toContain(SECRET_CANARY);
  });

  it("Delete sends null for the name and leaves an untouched name out; a template still using the deleted name stops the save", async () => {
    routes["GET /v1/connections/c1/webhook"] = () => json(200, view({ secret_value_names: ["topic", "key"], template: '{"text": {{text}}, "a": "{{secret:topic}}", "b": "{{secret:key}}"}' }));
    routes["GET /v1/apps/webhook/webhook-presets"] = () => json(200, presets);
    routes["PUT /v1/connections/c1/webhook"] = () => json(200, view());
    const onOpenChange = vi.fn();
    wrap(<WebhookEditDialog connectionId="c1" name="alerts" open onOpenChange={onOpenChange} />);
    await screen.findByLabelText("Template");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText("Limits");
    fireEvent.click(screen.getByRole("button", { name: "Delete key" }));
    expect(screen.getByText(/will be deleted/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    // The template still uses {{secret:key}}: back to the template, with the reason, and nothing sent.
    expect(await screen.findByText(/uses a secret value that is not in the list/)).toBeInTheDocument();
    expect(calls.some((c) => c.method === "PUT")).toBe(false);
    fireEvent.change(screen.getByLabelText("Template"), { target: { value: '{"text": {{text}}, "a": "{{secret:topic}}"}' } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText("Limits");
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect((calls.find((c) => c.method === "PUT")!.body as { secret_values: unknown }).secret_values).toEqual({ key: null });
  });

  it("regenerating signing shows the new secret once and closing drops it", async () => {
    routes["GET /v1/connections/c1/webhook"] = () => json(200, view({ secret_value_names: [] }));
    routes["GET /v1/apps/webhook/webhook-presets"] = () => json(200, presets);
    routes["PUT /v1/connections/c1/webhook"] = () => json(200, { ...view({ secret_value_names: [] }), signing_secret: SIGNING });
    const onOpenChange = vi.fn();
    wrap(<WebhookEditDialog connectionId="c1" name="alerts" open onOpenChange={onOpenChange} />);
    await screen.findByLabelText("Template");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText("Limits");
    fireEvent.click(screen.getByLabelText("Make a new one"));
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    const once = await screen.findByTestId("signing-secret-once");
    expect(within(once).getByText(SIGNING)).toBeInTheDocument();
    expect((calls.find((c) => c.method === "PUT")!.body as { signing_secret: unknown }).signing_secret).toEqual({ mode: "generate" });
    expect(onOpenChange).not.toHaveBeenCalled();
    fireEvent.click(within(once).getByRole("button", { name: /I have saved it/ }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("checks a limit against the ceiling that applies, and maps a server refusal onto the field", async () => {
    routes["GET /v1/connections/c1/webhook"] = () => json(200, view({ secret_value_names: [] }));
    routes["GET /v1/apps/webhook/webhook-presets"] = () => json(200, presets);
    routes["PUT /v1/connections/c1/webhook"] = () => err(422, "webhook_config_invalid", { fields: [{ path: "/rate/per_day", code: "rate_above_ceiling" }] });
    wrap(<WebhookEditDialog connectionId="c1" name="alerts" open onOpenChange={vi.fn()} />);
    await screen.findByLabelText("Template");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText("Limits");
    fireEvent.change(screen.getByLabelText(/Sends per minute/), { target: { value: "31" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByText("This is above the limit of 30 that applies here.")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "PUT")).toBe(false);
    fireEvent.change(screen.getByLabelText(/Sends per minute/), { target: { value: "20" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByText("This is above the limit your organization or administrator allows.")).toBeInTheDocument();
  });
});

describe("Send test", () => {
  const sent = { status: 200, content_type: "text/html", body: "<script>window.hacked = 1</script>ok", truncated: true, delivery_id: "7d2c1d9e-0000-4000-8000-000000000001" };

  it("asks before it really calls the webhook; a dry run needs no question", async () => {
    routes["GET /v1/connections/c1/webhook"] = () => json(200, view());
    routes["POST /v1/connections/c1/webhook/test"] = (b) =>
      b && b.dry_run ? json(200, { method: "POST", content_type: "application/json", headers: ["X-Key"], body: '{"text":"Deploy 42 failed"}' }) : json(200, sent);
    wrap(<SendTestDialog connectionId="c1" name="alerts" open onOpenChange={vi.fn()} />);
    await screen.findByLabelText("text");
    // The values form starts from the examples.
    expect(screen.getByLabelText("text")).toHaveValue("Deploy 42 failed");

    fireEvent.click(screen.getByRole("button", { name: "Dry run" }));
    expect(await screen.findByText("Dry run: nothing was sent.")).toBeInTheDocument();
    expect(calls.filter((c) => c.method === "POST")[0]!.body).toEqual({ values: { text: "Deploy 42 failed" }, dry_run: true });

    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const confirm = await screen.findByRole("dialog", { name: /Send a test to alerts/ });
    expect(within(confirm).getByText("This really calls the webhook. The receiver will act on it. A test sent here goes out now; an agent's send waits for your approval.")).toBeInTheDocument();
    // Nothing was sent by pressing Send.
    expect(calls.filter((c) => c.method === "POST")).toHaveLength(1);
    fireEvent.click(within(confirm).getByRole("button", { name: "Send" }));
    await screen.findByText("The receiver answered 200.");
    expect(calls.filter((c) => c.method === "POST")[1]!.body).toEqual({ values: { text: "Deploy 42 failed" } });
  });

  it("shows the status, the delivery id and truncated; the body is text, never HTML", async () => {
    routes["GET /v1/connections/c1/webhook"] = () => json(200, view());
    routes["POST /v1/connections/c1/webhook/test"] = () => json(200, sent);
    wrap(<SendTestDialog connectionId="c1" name="alerts" open onOpenChange={vi.fn()} />);
    await screen.findByLabelText("text");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const confirm = await screen.findByRole("dialog", { name: /Send a test to alerts/ });
    fireEvent.click(within(confirm).getByRole("button", { name: "Send" }));
    await screen.findByText("The receiver answered 200.");
    expect(screen.getByText(sent.delivery_id)).toBeInTheDocument();
    expect(screen.getByText(/longer than 4 KiB/)).toBeInTheDocument();
    expect(screen.getByTestId("test-body").textContent).toBe(sent.body);
    expect(document.querySelector("script")).toBeNull();
    expect((window as unknown as { hacked?: number }).hacked).toBeUndefined();
  });

  it("names the wait of a rate limit and marks a value the server refuses", async () => {
    routes["GET /v1/connections/c1/webhook"] = () => json(200, view());
    let n = 0;
    routes["POST /v1/connections/c1/webhook/test"] = () => (n++ === 0 ? err(429, "webhook_rate_limited", { retry_after_secs: 12 }) : err(422, "invalid_slot", { slot: "text", rule: "max_length" }));
    wrap(<SendTestDialog connectionId="c1" name="alerts" open onOpenChange={vi.fn()} />);
    await screen.findByLabelText("text");
    fireEvent.click(screen.getByRole("button", { name: "Dry run" }));
    expect(await screen.findByText("This webhook's limit was reached. Try again in 12 seconds.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Dry run" }));
    expect(await screen.findByText("This is longer than the limit.")).toBeInTheDocument();
  });
});
