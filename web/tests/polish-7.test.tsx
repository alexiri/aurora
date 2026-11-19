import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { accountsSentence } from "@/components/accounts/accounts-list";
import { AppsTab, appStatus, isSwitchedOff } from "@/components/apps/apps-tab";
import { approvalsSentence, mergePending } from "@/components/approvals/approvals-view";
import { overviewSentence } from "@/components/overview/overview-view";
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
  it("says Ready, Connected (by account) and Needs <Name> first, and hides apps that are switched off", () => {
    expect(appStatus([app()])).toMatchObject({ kind: "ready", line: "Ready to connect" });
    expect(appStatus([app({ credential_free: true })]).line).toBe("Ready · no sign-in needed");
    expect(appStatus([app({ connection_count: 1, accounts: [{ id: "1", name: "acme-bot", agent_ids: [] }] })]).line).toBe("Connected · acme-bot");
    expect(appStatus([app({ connection_count: 2 })]).line).toBe("Connected · 2 accounts");
    const needs = app({ readiness: { state: "needs_admin", needs: ["oauth_client"], who_can: { action: "set_up", role: "org_admin", people: [{ name: "Alice Admin" }], you_can: false }, requested: { by_me: false, count: 0 } } });
    expect(appStatus([needs])).toMatchObject({ kind: "needs_admin", line: "Needs Alice Admin first", firstName: "Alice", askedByMe: false });
    expect(isSwitchedOff([app({ readiness: { state: "needs_admin", needs: [] } })])).toBe(true);
    expect(isSwitchedOff([needs])).toBe(false);
  });

  it("offers Connect, Connect another, or Ask <First name> (then Asked and Undo)", async () => {
    const posts: string[] = [];
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
        if (req.method === "POST" && url.pathname.endsWith("/setup-request")) posts.push(url.pathname);
        return json(200, {});
      }),
    );
    wrap(<AppsTab workspaceId="w1" orgId="o1" canOwn />);
    expect(await screen.findByRole("button", { name: "Connect Google Calendar" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect another Linear" })).toBeInTheDocument();
    expect(screen.queryByText("Switched off")).toBeNull();
    expect(screen.queryByText(/0 connections/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Ask Alice/ }));
    await vi.waitFor(() => expect(posts).toEqual(["/v1/workspaces/w1/apps/gdrive/setup-request"]));
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
});

describe("overview sentence", () => {
  it("says what needs the person, else that the agent is ready", () => {
    expect(overviewSentence(2, ["Daneel"])).toBe("2 things need you.");
    expect(overviewSentence(1, ["Daneel"])).toBe("1 thing needs you.");
    expect(overviewSentence(0, ["Daneel"])).toBe("Daneel is ready. Nothing needs you.");
    expect(overviewSentence(0, ["Daneel", "Giskard"])).toBe("Your agents are ready. Nothing needs you.");
  });
});
