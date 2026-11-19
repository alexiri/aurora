import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { byoSentence, aiModelsSentence } from "@/components/governance/org-governance";
import { postureWords, sourceWords } from "@/components/governance/byo-panel";
import { OrgAppsTable, buildRows, nextAvailability, providerSentence } from "@/components/apps/org-apps-table";
import { workspaceLabelOf } from "@/components/accounts/account-row";
import { json } from "./support/api";
import { wrap } from "./support/render";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme" }),
  usePathname: () => "/o/acme/apps",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));

// (P5-POLISH-9) The organization's pages: Apps as one table, the member views' sentences, BYO values in words, agent labels.

const app = (id: string, name: string, over: Record<string, unknown> = {}) =>
  ({ app_id: id, package_id: id, package_version: "1", name, description: "", available: true, connection_count: 0, has_dark_icon: false, serves_library: false, credential_free: false, app_restrictions: { custom_only: false, block_untooled: false }, accounts: [], ...over }) as never;
const setup = (id: string, name: string, clientKey: string | null) => ({
  app_id: id,
  name,
  methods: clientKey ? [{ id: "oauth", audience: "person", needs: ["oauth_client"], client_key: clientKey, ready: false }] : [{ id: "key", audience: "person", needs: [], ready: true }],
});

describe("apps table helpers", () => {
  const gap = (appId: string, clientKey: string) => ({ appId, name: appId, clientKey, tokenOnly: false });
  it("folds variants into one row named after the product with the hosts under it, and takes the worst status", () => {
    const fam = (v: string, bound: boolean) => ({ id: "github", name: "GitHub", variant_label: v, instance_bound: bound });
    const rows = buildRows([app("gh", "GitHub", { family: fam("Cloud", false) }), app("ghes", "GitHub Enterprise", { family: fam("Data Center", true) })], new Map([["ghes", gap("ghes", "github")]]));
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ name: "GitHub", hosts: "Cloud · Data Center" });
    expect(rows[0]!.gap?.appId).toBe("ghes");
  });
  it("names the providers that serve several apps", () => {
    const rows = buildRows([app("c", "Google Calendar"), app("d", "Google Drive"), app("g", "Gmail"), app("n", "Notion")], new Map([["c", gap("c", "google")], ["d", gap("d", "google")], ["g", gap("g", "google")], ["n", gap("n", "notion")]]));
    expect(providerSentence(rows)).toBe("One Google sign-in covers Calendar, Drive and Gmail.");
  });
  it("turning one app off while every app is on keeps the others on, and keeps workspace entries", () => {
    const next = nextAvailability({ posture: "open", allow: [{ app_id: "x", workspace_id: "w1" }] } as never, ["a", "b", "c"], "b", false);
    expect(next).toEqual({ posture: "restricted", allow: [{ app_id: "a" }, { app_id: "c" }, { app_id: "x", workspace_id: "w1" }] });
    const back = nextAvailability(next, ["a", "b", "c"], "b", true);
    expect(back.allow.map((e) => e.app_id).sort()).toEqual(["a", "b", "c", "x"]);
  });
});

describe("apps table", () => {
  it("shows status, Set up, a switch that saves at once with Undo, and an app's open setup request", async () => {
    const puts: unknown[] = [];
    let availability: unknown = { posture: "open", allow: [] };
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const req = input as Request;
        const url = new URL(req.url, "http://localhost");
        const key = `${req.method} ${url.pathname}`;
        if (key === "GET /v1/apps") return json(200, { items: [app("sentry", "Sentry"), app("linear", "Linear")] });
        if (key === "GET /v1/orgs/o1/app-availability") return json(200, availability);
        if (key === "PUT /v1/orgs/o1/app-availability") {
          availability = await req.json();
          puts.push(availability);
          return json(200, availability);
        }
        if (key === "GET /v1/apps/sentry/setup") return json(200, setup("sentry", "Sentry", null));
        if (key === "GET /v1/apps/linear/setup") return json(200, setup("linear", "Linear", "linear"));
        if (key === "GET /v1/orgs/o1/app-setup-requests")
          return json(200, { items: [{ id: "r1", app_id: "linear", app_name: "Linear", askers: [{ display_name: "Bob Builder", user_id: "b", workspace_id: "w", asked_at: "2026-11-19T10:00:00Z" }], needs: ["oauth_client"], status: "open", unavailable: false, org_id: "o1", created_at: "2026-11-19T10:00:00Z" }] });
        return json(200, {});
      }),
    );
    wrap(<OrgAppsTable orgId="o1" orgSlug="acme" canEdit />);
    const table = await screen.findByRole("table");
    expect(await within(table).findByText("Needs Linear sign-in")).toBeInTheDocument();
    expect(within(table).getByText("Ready")).toBeInTheDocument();
    expect(screen.getByTestId("apps-setup-note")).toHaveTextContent("1 app needs a sign-in app");
    // Bob's request on Linear's row.
    expect(within(table).getByTestId("setup-request")).toHaveTextContent("Bob asked");
    expect(within(table).getByRole("link", { name: /Set up Linear sign-in/ })).toHaveAttribute("href", expect.stringContaining("client_key=linear"));
    // The filter.
    fireEvent.click(screen.getByRole("radio", { name: /Needs setup 1/ }));
    expect(within(table).queryByText("Sentry")).toBeNull();
    fireEvent.click(screen.getByRole("radio", { name: /All 2/ }));
    // The switch saves on change (no Save button), then Undo restores.
    fireEvent.click(within(table).getByRole("switch", { name: "People may connect Sentry" }));
    await waitFor(() => expect(puts).toHaveLength(1));
    expect(puts[0]).toEqual({ posture: "restricted", allow: [{ app_id: "linear" }] });
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(await screen.findByTestId("apps-toast")).toHaveTextContent("Sentry switched off.");
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(puts).toHaveLength(2));
    expect(puts[1]).toEqual({ posture: "open", allow: [] });
  });
});

describe("member sentences, BYO words and agent labels", () => {
  it("says the AI models sentence for a key, for none, and for workspace keys not allowed", () => {
    expect(aiModelsSentence("Acme", ["Team key"], true)).toBe("Agents in Acme think with the organization's key, Team key. You can also add your own key in your workspace.");
    expect(aiModelsSentence("Acme", ["Team key"], false)).toBe("Agents in Acme think with the organization's key, Team key.");
    expect(aiModelsSentence("Acme", [], true)).toBe("Agents in Acme use keys you add in your workspace.");
  });
  it("says the BYO sentence and the stored values in words", () => {
    expect(byoSentence("owners_only", null)).toBe("Only workspace owners can run agents outside Aurora. Their tokens never expire unless an owner sets a limit.");
    expect(byoSentence("owners_only", 30)).toContain("last at most 30 days");
    expect(byoSentence("disabled", null)).toBe("Agents outside Aurora are turned off here.");
    expect(postureWords("owners_only")).toBe("Owners only");
    expect(postureWords("enabled")).toBe("Everyone");
    expect(sourceWords("instance")).toBe("Administration");
  });
  it("tells agents apart by their workspace outside their own: Bob's workspace", () => {
    const list = [{ id: "w2", name: "Bob Builder", personal_of: { id: "b", name: "Bob Builder" } }, { id: "w1", name: "Default", personal_of: null }];
    expect(workspaceLabelOf({ id: "w2", name: "Bob Builder" }, list, { id: "a" })).toBe("Bob's workspace");
    expect(workspaceLabelOf({ id: "w2", name: "Bob Builder" }, list, { id: "b" })).toBe("Your workspace");
    expect(workspaceLabelOf({ id: "w1", name: "Default" }, list, { id: "a" })).toBe("Default");
  });
});
