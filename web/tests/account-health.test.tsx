import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/o/acme/w/main/agents/ada",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { AccountHealth } from "@/components/accounts/account-health";
import { AccountsBanner, AgentAttentionChip, OverviewAccountsAttention } from "@/components/accounts/attention";
import { attentionCount, healthClause, healthLine, needsAttention, savedButText, testOutcomeText } from "@/lib/account-health";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

type Health = { state: string; detail?: string | null; checked_at?: string | null; last_used_at?: string | null };
const acct = (health: Health, o: Record<string, unknown> = {}) => ({
  ref: { type: "connection", id: "c1" },
  kind: "app",
  name: "GitHub account",
  app: { app_id: "github", name: "GitHub", custom_only: false },
  scope: "workspace",
  levels: [],
  health: { detail: null, checked_at: null, last_used_at: null, ...health },
  agents: [],
  can: { give_access: true, change_access: true, manage: true },
  given: true,
  ...o,
});
// eslint-disable-next-line @typescript-eslint/no-explicit-any -- fixtures are loose
const asAccount = (a: any) => a;

describe("the health line (14 §7)", () => {
  it("says each state in the one wording, with its fix", () => {
    const line = (h: Health) => healthLine(asAccount(acct(h)));
    expect(line({ state: "ok" }).text).toBe("Works");
    expect(healthLine(asAccount(acct({ state: "ok" })), "2 hours ago").text).toBe("Works · used 2 hours ago");
    expect(line({ state: "rejected", detail: "token" })).toMatchObject({ text: "GitHub no longer accepts this token", fix: { kind: "replace", label: "Replace token" } });
    expect(line({ state: "rejected", detail: "password" }).fix?.label).toBe("Replace password");
    expect(line({ state: "rejected", detail: "key" }).fix?.label).toBe("Replace key");
    expect(line({ state: "sign_in_expired" })).toMatchObject({ text: "The sign-in to GitHub expired", fix: { kind: "reconnect", label: "Reconnect" } });
    expect(line({ state: "unreachable", detail: "db.platform.example.org" })).toMatchObject({
      text: "Could not be reached (db.platform.example.org)",
      fix: { kind: "test", label: "Test" },
    });
    expect(line({ state: "disabled" })).toMatchObject({ text: "Turned off", fix: { kind: "turn_on", label: "Turn on" } });
    expect(line({ state: "unknown" }).text).toBe("Not checked yet");
  });

  it("names a website or an AI key by its own name, and never uses a raw reason", () => {
    const site = acct({ state: "rejected", detail: "key" }, { app: undefined, name: "Weather API" });
    expect(healthLine(asAccount(site)).text).toBe("Weather API no longer accepts this key");
    expect(healthLine(asAccount(acct({ state: "rejected", detail: "invalid_grant: raw upstream text" }))).text).toBe("GitHub no longer accepts this token");
  });

  it("counts the accounts that need attention, and a turned-off account is not one", () => {
    expect(attentionCount(0)).toBeNull();
    expect(attentionCount(1)).toBe("1 account needs attention");
    expect(attentionCount(3)).toBe("3 accounts need attention");
    expect(needsAttention(asAccount(acct({ state: "rejected" })))).toBe(true);
    expect(needsAttention(asAccount(acct({ state: "sign_in_expired" })))).toBe(true);
    expect(needsAttention(asAccount(acct({ state: "unreachable" })))).toBe(true);
    expect(needsAttention(asAccount(acct({ state: "disabled" })))).toBe(false);
    expect(needsAttention(asAccount(acct({ state: "ok" })))).toBe(false);
  });

  it("has the clauses of the banner, and the Test and post-connect sentences", () => {
    expect(healthClause(asAccount(acct({ state: "sign_in_expired" })))).toBe("the sign-in expired");
    expect(testOutcomeText("not_testable")).toBe("This account cannot be tested. Its state will show after an agent uses it.");
    expect(testOutcomeText("error")).toBe("The test could not run. Try again.");
    expect(savedButText("PostgreSQL", "db.platform.example.org")).toBe(
      "Saved, but PostgreSQL could not be reached (db.platform.example.org). Check the address or test again.",
    );
  });
});

describe("the row's health line", () => {
  it("shows the line and the fix, and calls it", () => {
    const onFix = vi.fn();
    render(<AccountHealth account={asAccount(acct({ state: "sign_in_expired" }))} onFix={onFix} />);
    expect(screen.getByText("The sign-in to GitHub expired")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Reconnect" }));
    expect(onFix).toHaveBeenCalledWith("reconnect");
  });

  it("shows no fix to someone who cannot make it, and a test outcome in place of the line", () => {
    const { rerender } = render(<AccountHealth account={asAccount(acct({ state: "rejected", detail: "token" }))} />);
    expect(screen.queryByRole("button")).toBeNull();
    rerender(<AccountHealth account={asAccount(acct({ state: "ok" }))} outcome="error" />);
    expect(screen.getByText("The test could not run. Try again.")).toBeTruthy();
  });
});

function mockAccounts(items: unknown[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = typeof input === "string" ? new Request(new URL(input, "http://localhost")) : (input as Request);
      const url = new URL(req.url, "http://localhost");
      const body = url.pathname.endsWith("/accounts") ? { items, next_cursor: null } : { error: { code: "not_found", message: "nf" } };
      return new Response(JSON.stringify(body), { status: url.pathname.endsWith("/accounts") ? 200 : 404, headers: { "content-type": "application/json" } });
    }),
  );
}
const wrap = (ui: React.ReactElement) => render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);

describe("where a broken account shows up", () => {
  it("counts the agent's broken accounts on its header and links to the Access tab", async () => {
    mockAccounts([acct({ state: "sign_in_expired" }), acct({ state: "rejected", detail: "token" }, { ref: { type: "connection", id: "c2" } }), acct({ state: "ok" }, { ref: { type: "connection", id: "c3" } })]);
    wrap(<AgentAttentionChip workspaceId="ws1" agentId="a1" accessHref="/o/acme/w/main/agents/ada?tab=access" />);
    const link = await screen.findByRole("link", { name: "2 accounts need attention" });
    expect(link.getAttribute("href")).toBe("/o/acme/w/main/agents/ada?tab=access");
  });

  it("shows nothing when every account works, and ignores an account not given to the agent", async () => {
    mockAccounts([acct({ state: "ok" }), acct({ state: "rejected" }, { ref: { type: "connection", id: "c9" }, given: false })]);
    wrap(<AgentAttentionChip workspaceId="ws1" agentId="a1" accessHref="/x" />);
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("puts a banner above the message box: the agent, the account, the line in lower case and the fix", async () => {
    mockAccounts([acct({ state: "sign_in_expired" })]);
    wrap(<AccountsBanner workspaceId="ws1" agent={{ id: "a1", name: "Daneel" }} base="/o/acme/w/main" />);
    expect(await screen.findByText(/Daneel can't use GitHub: the sign-in expired\./)).toBeTruthy();
    const fix = screen.getByRole("link", { name: "Reconnect" });
    expect(fix.getAttribute("href")).toBe("/o/acme/w/main/agents/a1?tab=access&reconnect=c1");
  });

  it("lists the workspace's broken accounts on the Overview with the row's line and fix, and nothing when all work", async () => {
    mockAccounts([
      acct({ state: "sign_in_expired" }),
      acct({ state: "rejected", detail: "password" }, { ref: { type: "connection", id: "c2" }, name: "analytics", app: { app_id: "postgresql", name: "PostgreSQL", custom_only: false } }),
      acct({ state: "ok" }, { ref: { type: "connection", id: "c3" } }),
    ]);
    const { unmount } = wrap(<OverviewAccountsAttention workspaceId="ws1" base="/o/acme/w/main" />);
    expect(await screen.findByRole("heading", { name: "2 accounts need attention" })).toBeTruthy();
    expect(screen.getByText(/: The sign-in to GitHub expired/)).toBeTruthy();
    expect(screen.getByText(/analytics/).parentElement?.textContent).toContain("PostgreSQL no longer accepts this password");
    expect(screen.getByRole("link", { name: "Reconnect" }).getAttribute("href")).toBe("/o/acme/w/main/apps?tab=accounts&reconnect=c1");
    expect(screen.getByRole("link", { name: "Replace password" }).getAttribute("href")).toBe("/o/acme/w/main/apps?tab=accounts");
    unmount();
    mockAccounts([acct({ state: "ok" })]);
    wrap(<OverviewAccountsAttention workspaceId="ws1" base="/o/acme/w/main" />);
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("heading")).toBeNull();
  });

  it("shows who can fix it instead of the fix to someone who cannot", async () => {
    mockAccounts([acct({ state: "rejected", detail: "token" }, { can: { give_access: false, change_access: false, manage: false } })]);
    wrap(<AccountsBanner workspaceId="ws1" agent={{ id: "a1", name: "Daneel" }} base="/o/acme/w/main" />);
    expect(await screen.findByText(/Daneel can't use GitHub: it no longer accepts this token\./)).toBeTruthy();
    expect(screen.queryByRole("link", { name: /Replace/ })).toBeNull();
  });
});
