import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme" }),
  useSearchParams: () => new URLSearchParams(""),
  usePathname: () => "/admin/apps",
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
}));

import { OutgoingWebhooksAdminSection } from "@/components/webhook/admin-webhook-section";
import { OutgoingWebhooksOrgSettings } from "@/components/webhook/org-webhook-settings";
import type { Org } from "@/lib/api/queries";

// P5-WEBHOOK-4: master stays deployable before P5-WEBHOOK-2. The organization's switch and the admin section render only
// when the server's responses carry what P5-WEBHOOK-2 adds.

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
let paths: string[] = [];
let index: { sections: { section: string }[]; locked: unknown[] };

beforeEach(() => {
  paths = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | Request) => {
      const path = new URL(typeof input === "string" ? input : input.url, "http://localhost").pathname;
      paths.push(path);
      if (path === "/v1/admin/settings") return json(200, index);
      if (path === "/v1/admin/settings/outgoing_webhooks") {
        return json(200, {
          section: "outgoing_webhooks", title: "Outgoing webhooks", description: "Limits.", generation: 1, updated_at: null, updated_by_label: null, settings: [],
        });
      }
      return json(404, { error: { code: "not_found", message: "nf" } });
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

const org = (settings: Record<string, unknown>): Org =>
  ({ id: "o1", slug: "acme", name: "Acme", my_role: "owner", settings: { members_can_create_workspaces: true, previews_enabled: false, calendar_invitations_enabled: true, ...settings } }) as unknown as Org;

describe("the organization's outgoing-webhook settings", () => {
  it("render nothing on today's organization (no outgoing-webhook keys)", () => {
    const { container } = wrap(<OutgoingWebhooksOrgSettings org={org({})} />);
    expect(container).toBeEmptyDOMElement();
  });
  it("render with the keys P5-WEBHOOK-2 returns, even when the limits are null", () => {
    wrap(<OutgoingWebhooksOrgSettings org={org({ outgoing_webhooks_enabled: true, outgoing_webhooks_per_minute_max: null, outgoing_webhooks_per_day_max: null })} />);
    expect(screen.getByLabelText("Allow outgoing webhooks in this organization")).toBeInTheDocument();
  });
});

describe("the outgoing_webhooks settings section", () => {
  it("renders nothing, and never requests the section, while the index does not list it", async () => {
    index = { sections: [{ section: "governance" }, { section: "llm" }], locked: [] };
    const { container } = wrap(<OutgoingWebhooksAdminSection />);
    await waitFor(() => expect(paths).toContain("/v1/admin/settings"));
    await new Promise((r) => setTimeout(r, 50));
    expect(container).toBeEmptyDOMElement();
    expect(paths).not.toContain("/v1/admin/settings/outgoing_webhooks");
  });
  it("renders nothing when the index request fails", async () => {
    index = undefined as never;
    vi.stubGlobal("fetch", vi.fn(async () => json(500, { error: { code: "internal", message: "x" } })));
    const { container } = wrap(<OutgoingWebhooksAdminSection />);
    await new Promise((r) => setTimeout(r, 50));
    expect(container).toBeEmptyDOMElement();
  });
  it("renders the section once the index lists it", async () => {
    index = { sections: [{ section: "governance" }, { section: "outgoing_webhooks" }], locked: [] };
    wrap(<OutgoingWebhooksAdminSection />);
    expect(await screen.findByRole("heading", { name: "Outgoing webhooks" })).toBeInTheDocument();
    expect(paths).toContain("/v1/admin/settings/outgoing_webhooks");
  });
});
