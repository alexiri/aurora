import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/agents",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { AgentsDefaultsView, ComputersView } from "@/components/admin/settings/operational-settings-views";
import { withFields } from "./support/settings-fields";

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- request bodies are inspected loosely
type Json = Record<string, any>;
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = typeof input === "string" ? new Request(new URL(input, "http://localhost")) : (input as Request);
      const url = new URL(req.url, "http://localhost");
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

const schema = (kind: string, o: Json = {}) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null, ...o });
const base = { locked: false, helm_key: "config.x", list: null, id_field: null, entries: null, default: null, secret: null };
const bodyOf = async (req: Request) => (await req.clone().json()) as Json;
const view = (ui: React.ReactElement) =>
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);

function section(id: string, title: string, settings: Json[], extra: unknown = null): Json {
  return withFields({ section: id, title, description: `${title} description.`, generation: 3, updated_at: null, updated_by_label: null, extra, settings });
}

const agents = () =>
  section(
    "agents",
    "Agents & sandboxes",
    [
      { ...base, key: "hosted.idle_stop_secs", value: 1800, source: "helm", locked: true, schema: schema("int") },
      { ...base, key: "hosted.turn_ceiling_secs", value: 21600, source: "default", default: 21600, schema: schema("int") },
    ],
    { runners: [] },
  );
const approvals = () =>
  section("approvals", "Approvals", [
    { ...base, key: "approvals.timeout_secs", value: 3600, source: "default", default: 3600, schema: schema("int") },
  ]);
const clean = { generation: 3, diff: [], loosening: [], affected_orgs: [], affected_orgs_total: 0, warnings: [] };

describe("Administration › Agents: stacked settings sections have one Save (P5-IA-3)", () => {
  it("shows one Save for the page and writes every changed section from it, a locked key staying read-only", async () => {
    const puts: Record<string, Json> = {};
    mockFetch({
      "GET /v1/admin/settings/agents": () => json(200, agents()),
      "GET /v1/admin/settings/approvals": () => json(200, approvals()),
      "POST /v1/admin/settings/agents/preview": () => json(200, clean),
      "POST /v1/admin/settings/approvals/preview": () => json(200, clean),
      "PUT /v1/admin/settings/agents": async (req) => {
        puts.agents = await bodyOf(req);
        return json(200, agents());
      },
      "PUT /v1/admin/settings/approvals": async (req) => {
        puts.approvals = await bodyOf(req);
        return json(200, approvals());
      },
    });
    view(<AgentsDefaultsView />);
    await screen.findByText("Words of approvals.timeout_secs");
    // One Save and one Discard for both sections; nothing to save yet.
    // The save bar is hidden until the first change (P5-REFINE-2); one Save for both sections once something changed.
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    fireEvent.change(document.getElementById("approvals.timeout_secs-c")!, { target: { value: "901" } });
    expect(screen.getAllByRole("button", { name: "Save" })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    // The Helm-locked deadline has no control.
    expect(document.getElementById("hosted.idle_stop_secs-c")).toBeNull();
    expect(screen.getByTestId("hosted.idle_stop_secs-locked-value").textContent).toBe("1800");

    fireEvent.change(document.getElementById("hosted.turn_ceiling_secs-c")!, { target: { value: "7200" } });
    fireEvent.change(document.getElementById("approvals.timeout_secs-c")!, { target: { value: "900" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts.approvals).toBeDefined());
    expect(puts.agents).toEqual({ generation: 3, values: { "hosted.turn_ceiling_secs": 7200 } });
    expect(puts.approvals).toEqual({ generation: 3, values: { "approvals.timeout_secs": 900 } });
    expect(await screen.findByText(/Saved\. Applied to every replica/)).toBeTruthy();
  });

  it("writes only the section that changed", async () => {
    const puts: string[] = [];
    mockFetch({
      "GET /v1/admin/settings/agents": () => json(200, agents()),
      "GET /v1/admin/settings/approvals": () => json(200, approvals()),
      "POST /v1/admin/settings/approvals/preview": () => json(200, clean),
      "PUT /v1/admin/settings/approvals": () => {
        puts.push("approvals");
        return json(200, approvals());
      },
      "PUT /v1/admin/settings/agents": () => {
        puts.push("agents");
        return json(200, agents());
      },
    });
    view(<AgentsDefaultsView />);
    await screen.findByText("Words of approvals.timeout_secs");
    fireEvent.change(document.getElementById("approvals.timeout_secs-c")!, { target: { value: "900" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Saved\. Applied/);
    expect(puts).toEqual(["approvals"]);
  });

  it("asks once, naming each section's consequences, and writes nothing until it is confirmed", async () => {
    const puts: string[] = [];
    const terminal = () =>
      section(
        "terminal",
        "Terminal",
        [{ ...base, key: "terminal.awake_cap_secs", value: 14400, source: "default", default: 14400, schema: schema("int") }],
        { available: true },
      );
    const previews = () =>
      section("previews", "Previews", [
        { ...base, key: "previews.shares_per_agent", value: 5, source: "default", default: 5, schema: schema("int") },
      ]);
    mockFetch({
      "GET /v1/admin/settings/terminal": () => json(200, terminal()),
      "GET /v1/admin/settings/previews": () => json(200, previews()),
      "POST /v1/admin/settings/terminal/preview": () => json(200, clean),
      "POST /v1/admin/settings/previews/preview": () =>
        json(200, { ...clean, loosening: [{ key: "previews.shares_per_agent", entry_id: null, reason: "more shares per agent" }] }),
      "PUT /v1/admin/settings/terminal": () => {
        puts.push("terminal");
        return json(200, terminal());
      },
      "PUT /v1/admin/settings/previews": () => {
        puts.push("previews");
        return json(200, previews());
      },
    });
    view(<ComputersView />);
    await screen.findByText("Words of previews.shares_per_agent");
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    fireEvent.change(document.getElementById("terminal.awake_cap_secs-c")!, { target: { value: "7200" } });
    fireEvent.change(document.getElementById("previews.shares_per_agent-c")!, { target: { value: "20" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("more shares per agent");
    expect(puts).toEqual([]);
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts).toEqual(["terminal", "previews"]));
  });

  it("keeps a failing section's draft and shows its error there, after the ones that saved", async () => {
    mockFetch({
      "GET /v1/admin/settings/agents": () => json(200, agents()),
      "GET /v1/admin/settings/approvals": () => json(200, approvals()),
      "POST /v1/admin/settings/agents/preview": () => json(200, clean),
      "POST /v1/admin/settings/approvals/preview": () => json(200, clean),
      "PUT /v1/admin/settings/agents": () => json(200, agents()),
      "PUT /v1/admin/settings/approvals": () => json(422, { error: { code: "settings_invalid", message: "The timeout is too long." } }),
    });
    view(<AgentsDefaultsView />);
    await screen.findByText("Words of approvals.timeout_secs");
    fireEvent.change(document.getElementById("hosted.turn_ceiling_secs-c")!, { target: { value: "7200" } });
    fireEvent.change(document.getElementById("approvals.timeout_secs-c")!, { target: { value: "99999999" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("The timeout is too long.")).toBeTruthy();
    expect((document.getElementById("approvals.timeout_secs-c") as HTMLInputElement).value).toBe("99999999");
    expect(screen.queryByText(/Saved\. Applied/)).toBeNull();
  });
});
