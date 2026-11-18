import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/ui/code-field", () => ({ CodeField: () => <textarea aria-label="Template" /> }));
vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme" }),
  useSearchParams: () => new URLSearchParams(""),
  usePathname: () => "/o/acme/apps",
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
}));

import { OutgoingWebhooksOrgSettings } from "@/components/webhook/org-webhook-settings";
import { WebhookCardLine } from "@/components/webhook/webhook-card-line";
import type { Org } from "@/lib/api/queries";

// P5-WEBHOOK-4 (C-912, C-913): the card's line (host, effective limit with its source, Edit, Send test; never the address) and
// the organization's switch and limits.

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
let calls: { method: string; path: string; body: unknown }[] = [];
let routes: Record<string, (body: unknown) => Response> = {};

beforeEach(() => {
  calls = [];
  routes = {};
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | Request, init?: RequestInit) => {
      const req = typeof input === "string" ? null : input;
      const path = new URL(typeof input === "string" ? input : input.url, "http://localhost").pathname;
      const method = init?.method ?? req?.method ?? "GET";
      let body: unknown = null;
      if (typeof init?.body === "string") body = JSON.parse(init.body);
      else if (req && method !== "GET") body = await req.clone().json().catch(() => null);
      calls.push({ method, path, body });
      const h = routes[`${method} ${path}`];
      return h ? h(body) : json(404, { error: { code: "not_found", message: "nf" } });
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

const view = (source: "instance" | "org" | "connection") => ({
  version: 1, method: "POST", content_type: "application/json", template: "{{payload}}", slots: [], headers: [], description: "", signing: false,
  rate: { per_minute: source === "connection" ? 5 : null, per_day: null }, preset: null,
  host: "hooks.slack.com", secret_value_names: [], effective_rate: { per_minute: 5, per_day: 500, source },
});

describe("the card line", () => {
  for (const [source, words] of [["instance", "the instance's limit"], ["org", "your organization's limit"], ["connection", "this webhook's own limit"]] as const) {
    it(`shows the effective limit and its source: ${source}`, async () => {
      routes["GET /v1/connections/c1/webhook"] = () => json(200, view(source));
      wrap(<WebhookCardLine connectionId="c1" name="alerts" />);
      expect((await screen.findByTestId("webhook-limit")).textContent).toBe(`5 a minute, 500 a day (${words})`);
      expect(screen.getByText("hooks.slack.com")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Edit alerts" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Send test to alerts" })).toBeInTheDocument();
    });
  }
  it("opens Edit and Send test", async () => {
    routes["GET /v1/connections/c1/webhook"] = () => json(200, view("org"));
    routes["GET /v1/apps/webhook/webhook-presets"] = () => json(200, []);
    wrap(<WebhookCardLine connectionId="c1" name="alerts" />);
    await screen.findByTestId("webhook-limit");
    fireEvent.click(screen.getByRole("button", { name: "Send test to alerts" }));
    expect(await screen.findByRole("dialog", { name: "Send test: alerts" })).toBeInTheDocument();
  });
});

const org = (settings: Record<string, unknown>, my_role: "owner" | "admin" = "owner"): Org =>
  ({ id: "o1", slug: "acme", name: "Acme", my_role, settings: { members_can_create_workspaces: true, previews_enabled: false, calendar_invitations_enabled: true, ...settings } }) as unknown as Org;

describe("the organization's switch and limits", () => {
  it("saves the switch and the two limits, an empty limit as null", async () => {
    routes["PATCH /v1/orgs/o1"] = () => json(200, {});
    wrap(<OutgoingWebhooksOrgSettings org={org({})} />);
    const sw = screen.getByLabelText("Allow outgoing webhooks in this organization");
    expect(sw).toBeChecked();
    fireEvent.click(sw);
    fireEvent.change(screen.getByLabelText(/Sends per minute/), { target: { value: "10" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByText("Settings saved.");
    expect(calls.find((c) => c.method === "PATCH")!.body).toEqual({
      settings: { outgoing_webhooks_enabled: false, outgoing_webhooks_per_minute_max: 10, outgoing_webhooks_per_day_max: null },
    });
  });
  it("starts from what the organization holds and checks the numbers", async () => {
    wrap(<OutgoingWebhooksOrgSettings org={org({ outgoing_webhooks_enabled: false, outgoing_webhooks_per_day_max: 100 })} />);
    expect(screen.getByLabelText("Allow outgoing webhooks in this organization")).not.toBeChecked();
    expect(screen.getByLabelText(/Sends per day/)).toHaveValue("100");
    fireEvent.change(screen.getByLabelText(/Sends per day/), { target: { value: "0" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByText("Enter a whole number of 1 or more, or leave empty.")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "PATCH")).toBe(false);
  });
  it("puts a refusal above the instance's ceiling on the limit field", async () => {
    routes["PATCH /v1/orgs/o1"] = () => json(422, { error: { code: "above_instance_ceiling", message: "x" } });
    wrap(<OutgoingWebhooksOrgSettings org={org({})} />);
    fireEvent.change(screen.getByLabelText(/Sends per minute/), { target: { value: "9999" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(screen.getByText("This is above the limit your administrator set for the whole instance.")).toBeInTheDocument());
  });
  it("is read-only for an admin who is not an owner", () => {
    wrap(<OutgoingWebhooksOrgSettings org={org({}, "admin")} />);
    expect(screen.getByLabelText("Allow outgoing webhooks in this organization")).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Save changes" })).toBeNull();
  });
});
