import { fireEvent, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { accountsSentence, orgAccountsSentence, soleAgentOf } from "@/components/accounts/accounts-list";
import { AppsTab, appStatus, isSwitchedOff } from "@/components/apps/apps-tab";
import { approvalsSentence, mergePending } from "@/components/approvals/approvals-view";
import { activityLine, overviewSentence } from "@/components/overview/overview-view";
import { json } from "./support/api";
import { wrap } from "./support/render";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  usePathname: () => "/o/acme/w/main/apps",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));

// (P5-POLISH-7) The sentences and statuses the workspace pages say, from state.

const app = (over: Record<string, unknown> = {}) =>
  ({
    app_id: "gcal", package_id: "gcal", package_version: "1", name: "Google Calendar", description: "Events", available: true, connection_count: 0,
    has_dark_icon: false, serves_library: false, credential_free: false, app_restrictions: { custom_only: false, block_untooled: false },
    accounts: [], readiness: { state: "ready", needs: [] }, ...over,
  }) as never;

describe("apps: one status line and the button that follows it", () => {
  it("says Ready, Connected (by account) and that an admin sets it up once, and hides apps that are switched off", () => {
    expect(appStatus([app()])).toMatchObject({ kind: "ready", line: "Ready to connect" });
    expect(appStatus([app({ credential_free: true })]).line).toBe("Ready · no sign-in needed");
    expect(appStatus([app({ connection_count: 1, accounts: [{ id: "1", name: "acme-bot", agent_ids: [] }] })]).line).toBe("Connected · acme-bot");
    expect(appStatus([app({ connection_count: 2 })]).line).toBe("Connected · 2 accounts");
    const needs = app({ readiness: { state: "needs_admin", needs: ["oauth_client"], who_can: { action: "set_up", role: "org_admin", people: [{ name: "Alice Admin" }], you_can: false }, requested: { by_me: false, count: 0 } } });
    expect(appStatus([needs])).toMatchObject({ kind: "needs_admin", line: "An admin sets up Google Calendar sign-in once", askedByMe: false });
    expect(isSwitchedOff([app({ readiness: { state: "needs_admin", needs: [] } })])).toBe(true);
    expect(isSwitchedOff([needs])).toBe(false);
  });

  it("offers Connect, Connect another, or Ask an admin (then Asked and Undo)", async () => {
    const posts: string[] = [];
    const bodies: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const req = input as Request;
        const url = new URL(req.url, "http://localhost");
        const key = `${req.method} ${url.pathname}`;
        if (key === "GET /v1/apps") {
          return json(200, {
            items: [
              app(),
              app({ app_id: "linear", name: "Linear", connection_count: 1, accounts: [{ id: "1", name: "main", agent_ids: [] }] }),
              app({ app_id: "gdrive", name: "Google Drive", readiness: { state: "needs_admin", needs: ["oauth_client"], who_can: { action: "set_up", role: "org_admin", people: [{ name: "Alice Admin" }], you_can: false }, requested: { by_me: false, count: 0 } } }),
              app({ app_id: "off", name: "Switched off", readiness: { state: "needs_admin", needs: [] } }),
            ],
          });
        }
        if (req.method === "POST" && url.pathname.endsWith("/setup-request")) {
          posts.push(url.pathname);
          bodies.push(await req.clone().json());
        }
        return json(200, {});
      }),
    );
    wrap(<AppsTab workspaceId="w1" orgId="o1" canOwn />);
    expect(await screen.findByRole("button", { name: "Connect Google Calendar" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect another Linear" })).toBeInTheDocument();
    expect(screen.queryByText("Switched off")).toBeNull();
    expect(screen.queryByText(/0 connections/)).toBeNull();
    // (workspace-9, D-417) Ask opens the optional reason; nothing is sent until Send request.
    fireEvent.click(screen.getByRole("button", { name: /Ask an admin/ }));
    expect(posts).toEqual([]);
    fireEvent.change(await screen.findByLabelText(/Why do you need it\?/), { target: { value: "For the Friday report" } });
    fireEvent.click(screen.getByRole("button", { name: "Send request" }));
    await vi.waitFor(() => expect(posts).toEqual(["/v1/workspaces/w1/apps/gdrive/setup-request"]));
    expect(bodies).toEqual([{ reason: "For the Friday report" }]);
  });
});

describe("approvals: one list", () => {
  it("merges the gateway's and the actions' requests newest first, and says how many wait", () => {
    const g = (id: string, at: string) => ({ id, created_at: at }) as never;
    const merged = mergePending([g("g1", "2026-11-19T10:00:00Z"), g("g2", "2026-11-19T12:00:00Z")], [g("a1", "2026-11-19T11:00:00Z")]);
    expect(merged.map((m) => `${m.kind}:${m.item.id}`)).toEqual(["gateway:g2", "action:a1", "gateway:g1"]);
    expect(approvalsSentence(0)).toBe("Nothing is waiting for you.");
    expect(approvalsSentence(1)).toBe("1 request is waiting for you.");
    expect(approvalsSentence(3)).toBe("3 requests are waiting for you.");
  });
});

describe("accounts sentence", () => {
  it("counts the accounts, and an account that needs you takes precedence", () => {
    expect(accountsSentence(1, 0)).toBe("1 account.");
    expect(accountsSentence(3, 0)).toBe("3 accounts.");
    expect(accountsSentence(3, 1)).toBe("1 account needs you.");
    expect(accountsSentence(3, 2)).toBe("2 accounts need you.");
  });

  it("says the organization's accounts are shared with everyone in it", () => {
    expect(orgAccountsSentence(1, 0, "Users")).toBe("1 account, shared with everyone in Users.");
    expect(orgAccountsSentence(2, 1, "Users")).toBe("1 account needs you.");
  });

  it("says which agent uses them when one agent uses every account", () => {
    expect(accountsSentence(2, 0, "Daneel")).toBe("2 accounts. Daneel uses both.");
    expect(accountsSentence(1, 0, "Daneel")).toBe("1 account. Daneel uses it.");
    expect(accountsSentence(3, 0, "Daneel")).toBe("3 accounts. Daneel uses all of them.");
    expect(accountsSentence(2, 1, "Daneel")).toBe("1 account needs you.");
    const use = (...ids: string[]) => ({ agents: ids.map((id) => ({ agent: { id, name: id === "a1" ? "Daneel" : "Giskard" } })) }) as never;
    expect(soleAgentOf([use("a1"), use("a1")])).toBe("Daneel");
    expect(soleAgentOf([use("a1"), use("a2")])).toBeUndefined();
    expect(soleAgentOf([use("a1"), use()])).toBeUndefined();
  });
});

describe("overview activity line", () => {
  it("is a sentence, not a request line", () => {
    expect(activityLine({ decision: "allow", tool_id: "create_issue", host: "api.x.io" }, "Daneel", "Team tracker")).toBe("Daneel used create issue in Team tracker.");
    expect(activityLine({ decision: "allow", tool_id: null, host: "api.x.io" }, "Daneel")).toBe("Daneel reached api.x.io.");
    expect(activityLine({ decision: "blocked", tool_id: null, host: "evil.io" }, "Daneel")).toBe("Daneel tried to reach evil.io.");
  });
});

describe("overview sentence", () => {
  it("says what needs the person, else that the agent is ready", () => {
    expect(overviewSentence(2, ["Daneel"])).toBe("2 things need you.");
    expect(overviewSentence(1, ["Daneel"])).toBe("1 thing needs you.");
    expect(overviewSentence(0, ["Daneel"])).toBe("Daneel is ready. Nothing needs you.");
    expect(overviewSentence(0, ["Daneel", "Giskard"])).toBe("Your agents are ready. Nothing needs you.");
  });
});

describe("apps: groups, search and sort (workspace-9)", () => {
  const needsAdmin = (over: Record<string, unknown>) =>
    app({ readiness: { state: "needs_admin", needs: ["oauth_client"], requested: { by_me: false, count: 0 } }, ...over });
  function stubApps() {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const req = input as Request;
        const url = new URL(req.url, "http://localhost");
        if (`${req.method} ${url.pathname}` === "GET /v1/apps") {
          return json(200, {
            items: [
              app({ app_id: "zed", name: "Zed", description: "Editor" }),
              app({ app_id: "aws", name: "AWS", description: "S3 and EC2" }),
              app({ app_id: "linear", name: "Linear", connection_count: 1, accounts: [{ id: "1", name: "main", agent_ids: [] }] }),
              needsAdmin({ app_id: "gmail", name: "Gmail", package_id: "google-mail", description: "Mail" }),
            ],
          });
        }
        return json(200, {});
      }),
    );
  }
  const names = (list: HTMLElement) => within(list).getAllByRole("listitem").map((li) => within(li).getAllByRole("link").map((a) => a.textContent)[0] ?? li.textContent);

  it("draws Connected, Ready to connect and Needs an admin first as grids, A to Z, with the admin line in role words", async () => {
    stubApps();
    wrap(<AppsTab workspaceId="w1" orgId="o1" canOwn />);
    const ready = await screen.findByRole("list", { name: "Apps: Ready to connect" });
    expect(names(ready).map((n) => n?.slice(0, 3))).toEqual(["AWS", "Zed"]);
    expect(ready.className).toMatch(/grid/);
    expect(ready).toHaveClass("@2xl:grid-cols-2", "@4xl:grid-cols-3", "@7xl:grid-cols-4"); // the shared CardGrid (D-439)
    expect(within(screen.getByRole("list", { name: "Apps: Connected" })).getByText("Connected · main")).toBeInTheDocument();
    const blocked = screen.getByRole("list", { name: "Apps: Needs an admin first" });
    expect(within(blocked).getByText("An admin sets up Google sign-in once")).toBeInTheDocument();
    expect(within(blocked).getByRole("button", { name: "Ask an admin to set up Gmail" })).toBeInTheDocument();
  });

  it("search narrows the groups, drops empty ones, and says when nothing matches", async () => {
    stubApps();
    wrap(<AppsTab workspaceId="w1" orgId="o1" canOwn />);
    const box = await screen.findByRole("searchbox", { name: "Search apps" });
    fireEvent.change(box, { target: { value: "s3" } });
    expect(screen.getByRole("list", { name: "Apps: Ready to connect" })).toHaveTextContent("AWS");
    expect(screen.queryByRole("list", { name: "Apps: Connected" })).toBeNull();
    expect(screen.queryByRole("list", { name: "Apps: Needs an admin first" })).toBeNull();
    fireEvent.change(box, { target: { value: "nope" } });
    expect(screen.getByText("No app matches “nope”.")).toBeInTheDocument();
  });
});
