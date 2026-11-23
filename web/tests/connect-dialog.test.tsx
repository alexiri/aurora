import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AppsTab } from "@/components/apps/apps-tab";
import { AccountsList } from "@/components/accounts/accounts-list";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import RedirectPage from "../src/app/o/[org]/w/[ws]/apps/[app]/connect/page";
import { json } from "./support/api";

// FIX-149 (D-272, C-842): Connect and Reconnect are real dialogs on the page; every dialog has a close icon; the OAuth
// sign-in is the one separate window, and its result comes back over a BroadcastChannel.

let search = "";
const replace = vi.fn();
vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main", app: "github" }),
  useSearchParams: () => new URLSearchParams(search),
  usePathname: () => "/o/acme/w/main/apps/github/connect",
  useRouter: () => ({ replace, push: vi.fn() }),
}));
// "Give agents access" has its own tests (credential-access); here it only has to appear and be dismissable.
vi.mock("@/components/credentials/give-access", () => ({
  GiveAccessPanel: ({ onDismiss, subject }: { onDismiss: () => void; subject: { scopes: string[] } }) => (
    <div>
      <p>give-access scopes:{subject.scopes.join(",")}</p>
      <button onClick={onDismiss}>Skip</button>
    </div>
  ),
  AccessDialog: () => null,
}));

/** A same-process BroadcastChannel: a message reaches every other open channel with the same name. */
class FakeChannel {
  static open = new Set<FakeChannel>();
  static sent: unknown[] = [];
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  constructor(public name: string) {
    FakeChannel.open.add(this);
  }
  postMessage(data: unknown) {
    FakeChannel.sent.push(data);
    for (const c of FakeChannel.open) if (c !== this && c.name === this.name) c.onmessage?.({ data });
  }
  close() {
    FakeChannel.open.delete(this);
  }
}
const emit = (data: unknown) => act(() => new FakeChannel("aurora-connect").postMessage(data));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return { qc, ...render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>) };
}

const openSpy = vi.fn();
beforeEach(() => {
  vi.stubGlobal("BroadcastChannel", FakeChannel);
  vi.stubGlobal("open", openSpy);
  // jsdom has no `matchMedia`; the journey's frame (`fixedHeight`) asks whether the viewport is wide.
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query.includes("min-width"), media: query, onchange: null, addEventListener: () => undefined, removeEventListener: () => undefined,
    addListener: () => undefined, removeListener: () => undefined, dispatchEvent: () => false,
  }));
  window.history.replaceState(null, "", "/o/acme/w/main/credentials");
});
afterEach(() => {
  search = "";
  replace.mockReset();
  openSpy.mockReset();
  FakeChannel.open.clear();
  FakeChannel.sent = [];
  vi.useRealTimers();
});

describe("every dialog has a close icon", () => {
  it("DialogContent renders Close, which closes; Escape still closes", () => {
    const onOpenChange = vi.fn();
    render(
      <Dialog open onOpenChange={onOpenChange}>
        <DialogContent title="Hello">body</DialogContent>
      </Dialog>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    onOpenChange.mockClear();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("ConfirmDialog and FormDialog show it, and show only one close control in the header", () => {
    render(
      <>
        <ConfirmDialog open onOpenChange={() => {}} title="Sure?" description="It stops working now." confirmLabel="Yes" onConfirm={() => {}} />
        <FormDialog open onOpenChange={() => {}} title="Fill" submitLabel="Save" onSubmit={() => {}}>
          <input aria-label="x" />
        </FormDialog>
      </>,
    );
    for (const d of screen.getAllByRole("dialog", { hidden: true })) {
      expect(within(d).getAllByRole("button", { name: "Close", hidden: true })).toHaveLength(1);
    }
  });
});

const APP_SUMMARY = {
  app_id: "github", name: "GitHub", package_id: "github", package_version: "2.0.0", description: "d", available: true,
  connection_count: 0, has_dark_icon: false,
};
const field = { name: "access_token", label: "Token", type: "secret", store: "credentials", required: true };
const method = (over: Record<string, unknown>) => ({
  kind: "fields", label: null, description: null, recommended: false, fields: [], scopes: [], default_scopes: [],
  access_levels: [], probe_scopes: [], client_key: null, callback: null, audience: "person",
  scope_presets: { read: [], write: [] }, show_context: [], ...over,
});
const planMethod = (id: string, kind: string, over: Record<string, unknown> = {}) => ({
  id, kind, label: id, description: null, recommended: false, audience: "person", needs: [], ready: true,
  scope_presets: { read: [], write: [] }, ...over,
});
const PLAN = { app_id: "github", available: true, suggested_method: "token", methods: [planMethod("token", "fields"), planMethod("oauth", "oauth2")] };
const DETAIL = {
  ...APP_SUMMARY, category: "development", api_docs: null, connect_note: null, label_hint: null,
  connect: [
    method({ id: "token", label: "Token", fields: [field], recommended: true }),
    method({ id: "oauth", kind: "oauth2", label: "Sign in", client_key: "gh", callback: "query" }),
  ],
  tools: [], hosts: [], agent_instructions: null, credentials: [], metadata: [], oauth_clients: [], instances: null,
};
const CONN = {
  id: "c1", org_id: "o1", workspace_id: "w1", scope: "workspace", app_id: "github", package_id: "github", app_version: "2",
  label: "me@acme.io", status: "active", status_reason: null, scopes: ["repo"], missing_scopes: [], metadata: {},
  connect_method: "oauth", tenant_host: null, oauth_client_id: null, credentials_expires_at: null, connected_by: null,
  last_used_at: null, created_at: "", updated_at: "", injecting: [], access: "write",
};

/** The accounts-list row of CONN (C-917). */
const ACCOUNT = {
  ref: { type: "connection", id: "c1" }, kind: "app", name: "me@acme.io", scope: "workspace", levels: ["read", "ask", "act"],
  app: { app_id: "github", name: "GitHub", custom_only: false }, health: { state: "not_used" }, agents: [],
  can: { give_access: true, change_access: true, manage: true },
};
const list = () => <AccountsList scope={{ kind: "workspace", id: "w1" }} orgId="o1" canOwn />;

interface Call {
  method: string;
  path: string;
  body: unknown;
}
function stub(extra: Record<string, (c: Call) => Response> = {}) {
  const calls: Call[] = [];
  const routes: Record<string, (c: Call) => Response> = {
    "GET /v1/apps": () => json(200, { items: [APP_SUMMARY], next_cursor: null }),
    "GET /v1/apps/github": () => json(200, DETAIL),
    "GET /v1/apps/github/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/apps/github/connect-plan": () => json(200, PLAN),
    "GET /v1/workspaces/w1/oauth-clients": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/workspaces/w1/connections": () => json(200, { items: [CONN], next_cursor: null }),
    "GET /v1/workspaces/w1/accounts": () => json(200, { items: [ACCOUNT], next_cursor: null }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [], next_cursor: null }),
    ...extra,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url);
      const c: Call = { method: input.method, path: url.pathname, body: input.method === "GET" ? null : await input.json().catch(() => null) };
      calls.push(c);
      const h = routes[`${c.method} ${c.path}`];
      return h ? h(c) : json(404, { error: { code: "not_found", message: "x" } });
    }),
  );
  return calls;
}

const apps = () => <AppsTab workspaceId="w1" orgId="o1" canOwn />;

describe("Connect on the Apps tab", () => {
  it("opens a dialog named Connect <App> on the page and never opens a window", async () => {
    stub();
    wrap(apps());
    fireEvent.click(await screen.findByRole("button", { name: "Connect GitHub" }));
    const dialog = await screen.findByRole("dialog", { name: "Connect GitHub" });
    expect(await within(dialog).findByRole("button", { name: "Close" })).toBeInTheDocument();
    expect(openSpy).not.toHaveBeenCalled();
  });

  it("a fields method: the token, Connect, missing permissions and the Done sentence; Close refreshes the lists", async () => {
    const calls = stub({
      "POST /v1/apps/github/connect": () => json(200, { kind: "connected", connection: { ...CONN, id: "c9", scopes: ["repo"] }, grants: [], missing_scopes: ["admin"] }),
    });
    wrap(apps());
    fireEvent.click(await screen.findByRole("button", { name: "Connect GitHub" }));
    fireEvent.change(await screen.findByLabelText("Token"), { target: { value: "ghp_x" } });
    const before = calls.filter((c) => c.path === "/v1/apps").length;
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByText(/Some requested permissions were not granted: admin/)).toBeInTheDocument();
    expect(screen.getByText("GitHub is connected. No agent can use it yet.")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({ scope: "workspace", workspace_id: "w1", method_id: "token", fields: { access_token: "ghp_x" } });
    // Close, at the left of the footer (the dialog's own × is the other Close).
    fireEvent.click(screen.getAllByRole("button", { name: "Close" }).at(-1)!);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(calls.filter((c) => c.path === "/v1/apps").length).toBeGreaterThan(before));
    expect(openSpy).not.toHaveBeenCalled();
  });

  it("closing the dialog (×) drops the form: a reopened dialog has no secret in it", async () => {
    stub();
    wrap(apps());
    fireEvent.click(await screen.findByRole("button", { name: "Connect GitHub" }));
    fireEvent.change(await screen.findByLabelText("Token"), { target: { value: "ghp_secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    fireEvent.click(screen.getByRole("button", { name: "Connect GitHub" }));
    expect(await screen.findByLabelText("Token")).toHaveValue("");
  });
});

/** A stand-in for the sign-in window `window.open("about:blank")` returns. */
function fakeWindow() {
  const w = { closed: false, opener: window as unknown, location: { replace: vi.fn() }, focus: vi.fn(), close: vi.fn(() => { w.closed = true; }) };
  return w;
}

describe("OAuth sign-in in the dialog (C-842)", () => {
  async function openOauth() {
    fireEvent.click(await screen.findByRole("button", { name: "Connect GitHub" }));
    // The journey suggests the token; signing in is the other way that works, offered at the top ("Sign in with:").
    fireEvent.click(await screen.findByRole("radio", { name: /Sign in/ }));
  }

  it("opens about:blank synchronously on the click, then cuts the opener before navigating to the provider", async () => {
    const w = fakeWindow();
    openSpy.mockReturnValue(w);
    let openerAtNavigation: unknown = "unset";
    w.location.replace.mockImplementation(() => { openerAtNavigation = w.opener; });
    stub({ "POST /v1/apps/github/connect": () => json(200, { kind: "redirect", url: "https://provider.example/authorize?state=s" }) });
    wrap(apps());
    await openOauth();
    fireEvent.click(screen.getByRole("button", { name: "Sign in to GitHub" }));
    // synchronously: nothing was fetched yet, the window is already open and blank
    expect(openSpy).toHaveBeenCalledWith("about:blank", "aurora-sign-in", expect.stringContaining("popup"));
    expect(w.location.replace).not.toHaveBeenCalled();
    await waitFor(() => expect(w.location.replace).toHaveBeenCalledWith("https://provider.example/authorize?state=s"));
    expect(openerAtNavigation).toBeNull();
    expect(await screen.findByText("Waiting for you to sign in to GitHub…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open the sign-in window again" })).toBeInTheDocument();
  });

  it("a connected result for this app moves on to Done and is acknowledged", async () => {
    const w = fakeWindow();
    openSpy.mockReturnValue(w);
    stub({ "POST /v1/apps/github/connect": () => json(200, { kind: "redirect", url: "https://provider.example/a" }) });
    wrap(apps());
    await openOauth();
    fireEvent.click(screen.getByRole("button", { name: "Sign in to GitHub" }));
    await screen.findByText("Waiting for you to sign in to GitHub…");
    // another app's result is ignored
    await emit({ type: "aurora:connected", app_id: "other", connection_id: "x", missing_scopes: [] });
    expect(screen.queryByText(/is connected/)).toBeNull();
    await emit({ type: "aurora:connected", app_id: "github", connection_id: "c1", missing_scopes: [], granted_agent_ids: [] });
    expect(await screen.findByText("GitHub is connected. No agent can use it yet.")).toBeInTheDocument();
    expect(FakeChannel.sent).toContainEqual({ type: "aurora:connect-ack", connection_id: "c1" });
    fireEvent.click(screen.getAllByRole("button", { name: "Close" }).at(-1)!);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("a failed result shows the error, acknowledges it and returns to the form", async () => {
    openSpy.mockReturnValue(fakeWindow());
    stub({ "POST /v1/apps/github/connect": () => json(200, { kind: "redirect", url: "https://provider.example/a" }) });
    wrap(apps());
    await openOauth();
    fireEvent.click(screen.getByRole("button", { name: "Sign in to GitHub" }));
    await screen.findByText("Waiting for you to sign in to GitHub…");
    await emit({ type: "aurora:connect-failed", error: "oauth_exchange_failed" });
    expect(await screen.findByText(/did not complete \(oauth_exchange_failed\)/)).toBeInTheDocument();
    expect(FakeChannel.sent).toContainEqual({ type: "aurora:connect-ack", error: "oauth_exchange_failed" });
    expect(screen.getByRole("button", { name: "Sign in to GitHub" })).toBeInTheDocument();
  });

  it("a refused sign-in says what the provider answered (FIX-232, C-1158)", async () => {
    openSpy.mockReturnValue(fakeWindow());
    stub({ "POST /v1/apps/github/connect": () => json(200, { kind: "redirect", url: "https://provider.example/a" }) });
    wrap(apps());
    await openOauth();
    fireEvent.click(screen.getByRole("button", { name: "Sign in to GitHub" }));
    await screen.findByText("Waiting for you to sign in to GitHub…");
    await emit({
      type: "aurora:connect-failed",
      error: "oauth_exchange_failed",
      reason: { stage: "authorize", error: "invalid_request", provider_code: "AADSTS50194" },
    });
    expect(
      await screen.findByText(
        "The sign-in did not complete (oauth_exchange_failed). The provider refused the sign-in: AADSTS50194, invalid_request.",
      ),
    ).toBeInTheDocument();
  });

  it("a sign-in window closed without a result returns to the form", async () => {
    const w = fakeWindow();
    openSpy.mockReturnValue(w);
    stub({ "POST /v1/apps/github/connect": () => json(200, { kind: "redirect", url: "https://provider.example/a" }) });
    wrap(apps());
    await openOauth();
    fireEvent.click(screen.getByRole("button", { name: "Sign in to GitHub" }));
    await screen.findByText("Waiting for you to sign in to GitHub…");
    w.closed = true;
    expect(await screen.findByText("Sign-in was cancelled.", undefined, { timeout: 3000 })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Sign in to GitHub" })).toBeInTheDocument();
  });

  it("a blocked window says so, sends nothing, and offers a retry", async () => {
    openSpy.mockReturnValue(null);
    const calls = stub();
    wrap(apps());
    await openOauth();
    fireEvent.click(screen.getByRole("button", { name: "Sign in to GitHub" }));
    expect(await screen.findByText(/browser blocked the sign-in window/)).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST")).toBe(false);
    openSpy.mockReturnValue(fakeWindow());
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
  });

  it("an error from the connect call closes the sign-in window and is shown in the dialog", async () => {
    const w = fakeWindow();
    openSpy.mockReturnValue(w);
    stub({ "POST /v1/apps/github/connect": () => json(422, { error: { code: "validation_failed", message: "No OAuth client is configured." } }) });
    wrap(apps());
    await openOauth();
    fireEvent.click(screen.getByRole("button", { name: "Sign in to GitHub" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/OAuth client/);
    expect(w.close).toHaveBeenCalled();
  });
});

describe("Reconnect on the Accounts tab", () => {
  it("opens Reconnect <App> in place with the connection's method preselected", async () => {
    stub();
    wrap(list());
    fireEvent.keyDown(await screen.findByRole("button", { name: "More for me@acme.io" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Sign in again" }));
    const dialog = await screen.findByRole("dialog", { name: "Reconnect GitHub" });
    await waitFor(() => expect(within(dialog).getByRole("radio", { name: /Sign in/ })).toBeChecked());
    expect(openSpy).not.toHaveBeenCalled();
  });
});

describe("deep links", () => {
  it("?connect=<app> opens the dialog and closing it removes the parameter", async () => {
    search = "tab=apps&connect=github";
    window.history.replaceState(null, "", "/o/acme/w/main/credentials?tab=apps&connect=github");
    stub();
    wrap(apps());
    await screen.findByRole("dialog", { name: "Connect GitHub" });
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(window.location.search).toBe("?tab=apps");
  });

  it("an unknown app shows a notice, not an empty dialog", async () => {
    search = "tab=apps&connect=nope";
    stub();
    wrap(apps());
    expect(await screen.findByText(/“nope” is not installed/)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("?reconnect=<id> opens the Reconnect dialog on the Accounts tab", async () => {
    search = "tab=accounts&reconnect=c1";
    stub();
    wrap(list());
    await screen.findByRole("dialog", { name: "Reconnect GitHub" });
  });

  it("?reconnect=<id> of an account that is not in the list says so, and members are told who can", async () => {
    search = "tab=accounts&reconnect=nope";
    stub();
    wrap(list());
    expect(await screen.findByText(/no longer exists/)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
    cleanup();
    search = "tab=accounts&reconnect=c1";
    wrap(<AccountsList scope={{ kind: "workspace", id: "w1" }} orgId="o1" canOwn={false} />);
    expect(await screen.findByText(/Only a workspace owner can reconnect me@acme.io/)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("the old apps/<app>/connect route redirects, with and without reconnect", async () => {
    wrap(<RedirectPage />);
    await waitFor(() => expect(replace).toHaveBeenCalledWith("/o/acme/w/main/apps?tab=apps&connect=github"));
    cleanup();
    replace.mockReset();
    search = "reconnect=c1";
    wrap(<RedirectPage />);
    await waitFor(() => expect(replace).toHaveBeenCalledWith("/o/acme/w/main/apps?tab=accounts&reconnect=c1"));
  });
});
