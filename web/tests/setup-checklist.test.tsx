import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  useParams: () => ({}),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/admin",
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

import { SetupView, doneCount, setupComplete } from "@/components/admin/setup-view";
import type { SetupStatus } from "@/lib/api/onboarding";

type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
let calls: { key: string; body: unknown }[];
let routes: Record<string, Handler>;

beforeEach(() => {
  calls = [];
  routes = {};
  window.history.replaceState(null, "", "/");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const body = text ? JSON.parse(text) : undefined;
      const key = `${req.method} ${url.pathname}`;
      calls.push({ key, body });
      const h = routes[key];
      return h ? h(req, body) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const llmSecret = (id: string, name: string) => ({
  id, scope: "organization", kind: "llm", name, status: "active", host_pattern: null, path_pattern: null, injection: { kind: "bearer" }, value_source: "value", value_version: 1, llm: null, origin: null,
});
const placement = (over: Record<string, unknown> = {}) => ({
  saved: true, signup_mode: "open", mode: "shared_org", default_org: { id: "o1", slug: "users", name: "Users" }, default_org_role: "member",
  shared_workspaces: [], personal_workspace: { enabled: true, role: "owner", name_template: "{sub}|{email_local}" },
  starter_grants: [], personal_orgs: { count: 0, items: [] }, warnings: [], updated_at: "2026-11-10T08:00:00Z", updated_by: "u9", ...over,
});
const check = (id: string, status: string, reason: string, message: string, extra: Record<string, unknown> = {}) => ({ id, status, reason, message, value: null, helm: null, ...extra });

/** A fresh dev install after sign-in and placement: no AI key, no e-mail. */
const setupStatus = (over: Record<string, unknown> = {}): SetupStatus => ({
  placement_saved: true,
  personal_workspaces_without_starter: 0,
  you: { provider: "dev", provider_name: "Development sign-in", role_claim: null, values: [], granted: [] },
  steps: [
    { id: "roles", status: "ok", reason: "roles_ok", checks: [] },
    { id: "placement", status: "ok", reason: "placement_saved", checks: [] },
    { id: "llm", status: "todo", reason: "no_org_key", checks: [check("org_key", "todo", "no_org_key", "Users has no LLM key.")] },
    { id: "runners", status: "ok", reason: "runners_ok", checks: [check("runners", "ok", "runner_online", "1 runner(s) online.", { value: "1" }), check("gateway", "ok", "gateway_online", "The gateway is up.", { value: "1" })] },
    { id: "optional", status: "warning", reason: "email_not_configured", checks: [check("email", "warning", "email_not_configured", "E-mail is not set up.")] },
  ],
  ...over,
}) as unknown as SetupStatus;

const withStep = (id: string, patch: Record<string, unknown>) => (base = setupStatus()) =>
  ({ ...base, steps: base.steps.map((s) => (s.id === id ? { ...s, ...patch } : s)) }) as unknown as SetupStatus;
const keyOk = withStep("llm", { status: "ok", reason: "llm_ok", checks: [check("org_key", "ok", "org_key_present", "Users has an LLM key.")] });
const mailOk = withStep("optional", { status: "ok", reason: "optional", checks: [] });

const ME = { user: { id: "u1", display_name: "Ada", email: "ada@example.org", is_instance_admin: true }, memberships: [{ org: { id: "o1", slug: "users", name: "Users" }, role: "member", status: "active", workspaces: [{ id: "w1", slug: "ada", name: "Ada's workspace", personal: true }] }] };

function world(opts: { status?: ReturnType<typeof setupStatus>; replied?: boolean; keys?: unknown[]; uploaded?: unknown[] } = {}) {
  routes["GET /v1/admin/setup"] = () => json(200, opts.status ?? setupStatus());
  routes["GET /v1/admin/placement"] = () => json(200, placement());
  routes["GET /v1/me"] = () => json(200, ME);
  routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [{ id: "a1", handle: "daneel", name: "Daneel", kind: "hosted", is_default: true, frozen: null }], next_cursor: null });
  routes["GET /v1/agents/a1/conversations"] = () =>
    json(200, { items: opts.replied ? [{ id: "c1", agent_id: "a1", kind: "direct", last_seq: 3, created_at: "2026-11-10T09:00:00Z", last_activity_at: "2026-11-10T09:01:00Z" }] : [], next_cursor: null });
  routes["GET /v1/conversations/c1/events"] = () => json(200, { items: [{ seq: 2, type: "text", ts: "2026-11-10T09:00:05Z", payload: {} }], last_seq: 3 });
  routes["GET /v1/orgs/o1/secrets"] = () => json(200, { items: opts.keys ?? [] });
  routes["GET /v1/admin/plugins"] = () => json(200, { items: opts.uploaded ?? [], next_cursor: null });
  routes["GET /v1/llm/providers"] = () =>
    json(200, { providers: [{ id: "litellm", display_name: "LiteLLM", auth_modes: ["bearer"] }], endpoints: [{ id: "litellm", provider: "litellm", display_name: "AIGW", permitted: true, key_scopes: [] }] });
}

const row = async (id: string) => {
  await screen.findByRole("list", { name: "Setup steps" });
  return document.querySelector<HTMLElement>(`[data-step="${id}"]`)!;
};

describe("Administration › Overview: the five-step checklist (P5-POLISH-10, D-391)", () => {
  it("a fresh install shows five rows with one button each and no embedded form", async () => {
    world();
    wrap(<SetupView />);
    const steps = await screen.findByRole("list", { name: "Setup steps" });
    expect(within(steps).getAllByRole("listitem")).toHaveLength(5);
    expect(screen.getByText("2 of 5")).toBeInTheDocument();
    const signIn = await row("signin");
    expect(within(signIn).getByText("Development sign-in")).toBeInTheDocument();
    expect(within(signIn).getByRole("link", { name: "Review" })).toHaveAttribute("href", "/admin/people?tab=sign-in");
    const placed = await row("placement");
    expect(within(placed).getByText("The Users organization, each in their own workspace")).toBeInTheDocument();
    expect(within(placed).getByRole("link", { name: "Change" })).toHaveAttribute("href", "/admin/people?tab=new-users");
    expect(screen.queryByLabelText("Mode")).toBeNull();
    // No bare numbers under check lines, no approval step.
    expect(screen.queryByText(/approv(e|al) (the )?apps/i)).toBeNull();
    await waitFor(() => expect(within(document.querySelector<HTMLElement>('[data-step="try"]')!).getByRole("link", { name: "Open chat" })).toHaveAttribute("href", "/o/users/w/ada/agents/daneel?tab=chat"));
  });

  it("asks for no organization's keys before the placement names an organization (P5-POLISH-11)", async () => {
    world();
    routes["GET /v1/admin/placement"] = () => json(200, placement({ saved: false, default_org: null }));
    wrap(<SetupView />);
    await row("key");
    await waitFor(() => expect(calls.some((c) => c.key === "GET /v1/admin/placement")).toBe(true));
    expect(calls.filter((c) => c.key.includes("/secrets"))).toEqual([]);
  });

  it("the AI key row asks for a key until one exists, then says every new agent gets it", async () => {
    world();
    wrap(<SetupView />);
    const key = await row("key");
    expect(await within(key).findByText("Add an organization AI key")).toBeInTheDocument();
    expect(within(key).getByRole("button", { name: "Add key" })).toBeEnabled();
    expect(key.dataset.done).toBe("false");
    cleanup();
    world({ status: keyOk(), keys: [llmSecret("k9", "Users AI gateway")] });
    wrap(<SetupView />);
    const done = await row("key");
    await waitFor(() => expect(within(done).getByText("Users AI gateway · every new agent gets it")).toBeInTheDocument());
    expect(done.dataset.done).toBe("true");
  });

  it("Try it is done once the admin's agent has answered", async () => {
    world({ replied: true });
    wrap(<SetupView />);
    const tryIt = await row("try");
    await waitFor(() => expect(tryIt.dataset.done).toBe("true"));
    cleanup();
    world({ replied: false });
    wrap(<SetupView />);
    const again = await row("try");
    await waitFor(() => expect(calls.some((c) => c.key === "GET /v1/agents/a1/conversations")).toBe(true));
    expect(again.dataset.done).toBe("false");
  });

  it("the optional e-mail step is never done while it has an open item, and says optional", async () => {
    world();
    wrap(<SetupView />);
    const mail = await row("email");
    expect(within(mail).getByText(/optional/)).toBeInTheDocument();
    expect(mail.dataset.done).toBe("false");
    expect(within(mail).queryByText(/ready/i)).toBeNull();
    expect(within(mail).getByRole("link", { name: "Set up" })).toHaveAttribute("href", "/admin/platform?tab=email");
    cleanup();
    world({ status: mailOk() });
    wrap(<SetupView />);
    expect((await row("email")).dataset.done).toBe("true");
  });

  it("counts the steps done and counts them in doneCount/setupComplete", () => {
    const fresh = setupStatus();
    expect(doneCount(fresh, false)).toBe(2);
    expect(setupComplete(fresh, true)).toBe(false);
    const ready = mailOk(keyOk());
    expect(doneCount(ready, true)).toBe(5);
    expect(setupComplete(ready, true)).toBe(true);
    // E-mail is optional: the required steps alone complete setup.
    expect(setupComplete(keyOk(), true)).toBe(true);
    expect(setupComplete(keyOk(), false)).toBe(false);
  });

  it("uploaded apps waiting get one line under the checklist; apps shipped with Aurora never do", async () => {
    world({ uploaded: [{ id: "p1", origin: "uploaded", package_id: "x" }, { id: "p2", origin: "aurora", package_id: "y" }] });
    wrap(<SetupView />);
    const line = await screen.findByTestId("uploaded-waiting");
    expect(line).toHaveTextContent("1 uploaded app is waiting for review.");
    expect(within(line).getByRole("link", { name: "Open Apps" })).toHaveAttribute("href", "/admin/apps?tab=apps");
  });

  it("adding the AI key keeps the checklist, shows a toast with the count, and refetches the status", async () => {
    world();
    let added = false;
    routes["GET /v1/admin/setup"] = () => json(200, added ? keyOk() : setupStatus());
    routes["GET /v1/orgs/o1/secrets"] = () => json(200, { items: added ? [llmSecret("k10", "AIGW")] : [] });
    routes["POST /v1/orgs/o1/secrets"] = () => {
      added = true;
      return json(201, llmSecret("k10", "AIGW"));
    };
    routes["PUT /v1/admin/placement"] = () => json(200, placement());
    wrap(<SetupView />);
    const key = await row("key");
    await waitFor(() => expect(within(key).getByRole("button", { name: "Add key" })).toBeEnabled());
    fireEvent.click(within(key).getByRole("button", { name: "Add key" }));
    fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "AIGW" } });
    fireEvent.change(await screen.findByLabelText("Provider endpoint"), { target: { value: "litellm" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: "sk-test" } });
    // (P5-POLISH-11) The dialog's buttons are P5-POLISH-7's: Cancel and "Add key".
    const dialog = screen.getByRole("dialog", { name: "Add an organization AI key" });
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Add key" }));
    const toast = await screen.findByTestId("setup-toast");
    expect(toast).toHaveTextContent("AI key added. 3 of 5 done.");
    // Still the checklist, with the starter grant saved (C-922).
    expect(screen.getByRole("list", { name: "Setup steps" })).toBeInTheDocument();
    expect(screen.queryByTestId("admin-health")).toBeNull();
    expect(calls.find((c) => c.key === "PUT /v1/admin/placement")?.body).toMatchObject({ mode: "shared_org", starter_grants: [{ kind: "secret", id: "k10" }] });
  });
});

describe("Administration › Overview: the health view", () => {
  const complete = () => mailOk(keyOk());

  it("replaces the checklist only when every required step is done; the tab and Run setup again switch", async () => {
    world({ status: setupStatus(), replied: true });
    wrap(<SetupView />);
    // Key missing: the checklist, though the admin has chatted.
    await screen.findByRole("list", { name: "Setup steps" });
    expect(screen.queryByTestId("admin-health")).toBeNull();
    // The Health tab is always there.
    fireEvent.click(screen.getByRole("tab", { name: "Health" }));
    expect(await screen.findByTestId("admin-health")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Run setup again" }));
    expect(await screen.findByRole("list", { name: "Setup steps" })).toBeInTheDocument();
  });

  it("is the health view once the required steps are done, with one sentence per problem and the right number of runners", async () => {
    world({ status: keyOk(), replied: true });
    routes["GET /v1/admin/storage"] = () => json(200, { blob_bytes: 2048, blob_count: 4, attachment_bytes: 0, attachment_files: 0, purged_files: 0, computed_at: "2026-11-17T10:00:00Z", orgs: [], tables: [] });
    wrap(<SetupView />);
    const health = await screen.findByTestId("admin-health");
    expect(within(health).getByText(/Aurora is running\. E-mail is not set up\./)).toBeInTheDocument();
    expect(within(health).getAllByText("E-mail isn't set up. Aurora can't send approval or invitation e-mails.")).toHaveLength(1);
    expect(within(health).getByRole("link", { name: "Set up e-mail" })).toHaveAttribute("href", "/admin/platform?tab=email");
    const lines = within(health).getByRole("list", { name: "Health" });
    expect(within(lines).getByText("1 runner online.")).toBeInTheDocument();
    expect(await within(lines).findByText(/2\.0 KiB in 4 stored files/)).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Setup steps" })).toBeNull();
  });

  it("says nothing needs the admin when nothing does", async () => {
    world({ status: complete(), replied: true });
    wrap(<SetupView />);
    expect(await screen.findByText(/Aurora is running\. Nothing needs you\./)).toBeInTheDocument();
    expect(screen.queryByText("Needs you")).toBeNull();
  });
});
