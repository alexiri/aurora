import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConnectJourney } from "@/components/apps/connect-journey/connect-journey";
import { stepsOf, type JourneyShape } from "@/components/apps/connect-journey/logic";
import { json } from "./support/api";

// P5-REFINE-13 (access-4, access-5): the Outgoing webhook's journey is flat: App · Address · Message · Done, with Back and
// Next in the journey's footer (no wizard inside the wizard), and Done offers "Send a test" with the result in words.

vi.mock("@/components/ui/code-field", () => ({
  CodeField: ({ label, initial, docKey, onChange }: { label: string; initial: string; docKey: string | number; onChange: (t: string) => void }) => (
    <textarea key={docKey} aria-label={label} defaultValue={initial} onChange={(e) => onChange(e.target.value)} />
  ),
}));
vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(""),
  usePathname: () => "/o/acme/w/main/apps",
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
}));

const shape = (over: Partial<JourneyShape> = {}): JourneyShape => ({
  appGiven: false,
  agentFixed: false,
  noAgents: false,
  website: false,
  hasExisting: false,
  useExisting: false,
  hasChoice: false,
  noAgentChosen: false,
  ...over,
});

describe("the steps of the Outgoing webhook", () => {
  it("are App, Address, Message and Done for a fixed agent", () => {
    expect(stepsOf(shape({ agentFixed: true, webhook: true }))).toEqual(["app", "address", "message", "done"]);
  });
  it("put the access choice before the address when the app has one", () => {
    expect(stepsOf(shape({ agentFixed: true, webhook: true, hasChoice: true }))).toEqual(["app", "access", "address", "message", "done"]);
  });
  it("other apps keep Sign in", () => {
    expect(stepsOf(shape({ agentFixed: true }))).toEqual(["app", "signin", "done"]);
  });
});

const CONN = { id: "c9", scope: "workspace", app_id: "webhook", scopes: [], missing_scopes: [], label: "Ops alerts", status: "active" };
const slot = { name: "text", type: "string", required: true, description: "", example: "Deploy 42 failed" };
const cfg = {
  version: 1, method: "POST", content_type: "application/json", template: '{"text": {{text}}}', slots: [slot], headers: [], description: "", signing: false,
  rate: { per_minute: null, per_day: null }, preset: "slack",
};
const DETAIL = {
  app_id: "webhook", name: "Outgoing webhook", package_id: "webhook", package_version: "1.0.0", description: "d", available: true, connection_count: 0,
  has_dark_icon: false, category: "developer", api_docs: null, connect_note: null, label_hint: null, credential_free: false,
  app_restrictions: { custom_only: false, block_untooled: false },
  connect: [
    {
      id: "url", kind: "fields", label: "Webhook address", description: null, recommended: true, scopes: [], default_scopes: [], access_levels: [], probe_scopes: [], client_key: null,
      callback: null, audience: "person", scope_presets: { read: [], write: [] }, show_context: [],
      fields: [{ name: "name", label: "Name", type: "text", store: "metadata", required: true }, { name: "url", label: "URL", type: "secret", store: "credentials", required: true }],
    },
  ],
  first_prompt: null, tools: [{ id: "send", name: "Send", group: "send" }], hosts: [], agent_instructions: null, credentials: [], metadata: [], oauth_clients: [], instances: null,
};
const PLAN = { app_id: "webhook", available: true, suggested_method: "url", methods: [{ id: "url", kind: "fields", label: "url", description: null, recommended: true, audience: "person", needs: [], ready: true, scope_presets: { read: [], write: [] } }] };

let testAnswer: () => Response;
let viewSlots: unknown[] = [slot];
let calls: { method: string; path: string }[] = [];

beforeEach(() => {
  calls = [];
  viewSlots = [slot];
  testAnswer = () => json(200, { status: 200, content_type: "text/plain", body: "ok", truncated: false, delivery_id: "01a1b2c3-0000-4000-8000-000000000001" });
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query.includes("min-width"), media: query, onchange: null, addEventListener: () => undefined, removeEventListener: () => undefined,
    addListener: () => undefined, removeListener: () => undefined, dispatchEvent: () => false,
  }));
  const routes: Record<string, () => Response> = {
    "GET /v1/apps": () => json(200, { items: [{ ...DETAIL }], next_cursor: null }),
    "GET /v1/apps/webhook": () => json(200, DETAIL),
    "GET /v1/apps/webhook/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/apps/webhook/connect-plan": () => json(200, PLAN),
    "GET /v1/apps/webhook/webhook-presets": () => json(200, [{ id: "slack", name: "Slack", suggest_for_hosts: ["hooks.slack.com"], secret_value_names: [], config: cfg }]),
    "GET /v1/workspaces/w1/accounts": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Daneel", handle: "daneel", kind: "hosted" }], next_cursor: null }),
    "POST /v1/apps/webhook/connect": () => json(200, { kind: "connected", connection: CONN, grants: [{ agent_id: "a1" }], missing_scopes: [] }),
    "GET /v1/connections/c9/webhook": () => json(200, { ...cfg, slots: viewSlots, host: "hooks.example.com", secret_value_names: [], effective_rate: { per_minute: 30, per_day: 500, source: "org" } }),
    "POST /v1/connections/c9/webhook/test": () => testAnswer(),
    "DELETE /v1/connections/c9": () => json(204, null),
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string, init?: RequestInit) => {
      // The generated client passes a Request; the webhook module's own `call` passes a URL and an init.
      const url = new URL(typeof input === "string" ? input : input.url, "http://localhost");
      const method = init?.method ?? (typeof input === "string" ? "GET" : input.method);
      calls.push({ method, path: url.pathname });
      const h = routes[`${method} ${url.pathname}`];
      return h ? h() : json(404, { error: { code: "not_found", message: "x" } });
    }),
  );
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function open() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ConnectJourney workspaceId="w1" orgId="o1" agent={{ id: "a1", name: "Daneel" }} appId="webhook" onClose={vi.fn()} />
    </QueryClientProvider>,
  );
}

async function connectIt() {
  fireEvent.change(await screen.findByLabelText("Address to send to"), { target: { value: "https://hooks.example.com/ops-alerts" } });
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Ops alerts" } });
  fireEvent.click(screen.getByRole("button", { name: "Next" }));
  const preset = await screen.findByLabelText(/^Preset/);
  await screen.findByRole("option", { name: /Slack/ });
  fireEvent.change(preset, { target: { value: "slack" } });
  fireEvent.click(screen.getByRole("button", { name: "Connect" }));
}

describe("the flat journey", () => {
  it("a webhook whose values have no example asks for them in place and sends them (access-4)", async () => {
    viewSlots = [{ name: "count", type: "integer", required: true, description: "", example: null }];
    open();
    await connectIt();
    fireEvent.click(await screen.findByRole("button", { name: "Send a test" }));
    const field = await screen.findByLabelText("count");
    expect(screen.queryByText(/Open it from the account/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Send a test" }));
    expect(await screen.findByText("This value is required.")).toBeInTheDocument();
    fireEvent.change(field, { target: { value: "3" } });
    fireEvent.click(screen.getByRole("button", { name: "Send a test" }));
    expect(await screen.findByText("The test went through.")).toBeInTheDocument();
  });

  it("shows Address, Message and Done in the stepper and keeps Back and Next in the footer", async () => {
    open();
    await screen.findByLabelText("Address to send to");
    const steps = await screen.findByRole("list", { name: "Steps" });
    expect(within(steps).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["Address", "Message", "Done"]);
    expect(screen.getByText("Daneel sends a POST request here.")).toBeInTheDocument();
    // The footer is the journey's: Next is not inside the form, and there is no "Step 1 of 3".
    expect(screen.queryByText(/Step \d of/)).toBeNull();
    expect(screen.getByRole("button", { name: "Next" }).closest("form")).toBeNull();
  });

  it("Done names the account and offers Send a test; a 405 is one plain sentence and Edit address returns to Address", async () => {
    testAnswer = () => json(200, { status: 405, content_type: "text/html", body: "<script>window.hacked = 1</script><html>405</html>", truncated: false, delivery_id: "01a1b2c3-0000-4000-8000-000000000001" });
    open();
    await connectIt();
    expect(await screen.findByText("Not sure the address works?")).toBeInTheDocument();
    expect(screen.getByText(/Daneel can now/)).toHaveTextContent("Ops alerts");
    fireEvent.click(screen.getByRole("button", { name: "Send a test" }));
    expect(await screen.findByText("The test did not go through")).toBeInTheDocument();
    expect(screen.getByText("The address doesn't accept POST. Check the URL and try again.")).toBeInTheDocument();
    expect(screen.getByTestId("test-body").closest("details")).not.toHaveAttribute("open");
    expect(document.querySelector("script")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Edit address" }));
    // Back to Address with the name kept and the address empty (it is never shown again).
    expect(await screen.findByLabelText("Address to send to")).toHaveValue("");
    expect(screen.getByLabelText("Name")).toHaveValue("Ops alerts");
  });

  it("a test that goes through says so; Send test again sends again", async () => {
    open();
    await connectIt();
    fireEvent.click(await screen.findByRole("button", { name: "Send a test" }));
    expect(await screen.findByText("The test went through.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Send test again" }));
    await waitFor(() => expect(calls.filter((c) => c.path === "/v1/connections/c9/webhook/test")).toHaveLength(2));
  });
});
