import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";

import { AppsTab } from "@/components/apps/apps-tab";
import { DeploymentStep } from "@/components/apps/connect-dialog";
import { AppStep } from "@/components/apps/connect-journey/app-step";
import { SignInStep } from "@/components/apps/connect-journey/sign-in-step";
import { appDisplayName, isProductReady, productKey, productName, usableChoices } from "@/lib/api/app-name";
import { wrap } from "./support/render";
import { json } from "./support/api";

// FIX-135 (D-242, C-744): one catalogue entry per product. Apps of a family are one tile, the connect window asks where the
// product runs, and every list names an app "<family> · <variant>".

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useSearchParams: () => new URLSearchParams(),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
}));

const fam = (variant_label: string, instance_bound: boolean) => ({ id: "github", name: "GitHub", variant_label, instance_bound });
const summary = (app_id: string, name: string, family: ReturnType<typeof fam> | null, n: number) => ({
  app_id, name, package_id: "github", package_version: "2.2.0", description: `${name} d`, category: "development", available: true,
  connection_count: n, has_dark_icon: false, family,
  app_restrictions: { custom_only: false, full_allowed: true },
  serves_library: false,
});
const field = { name: "access_token", label: "Token", type: "secret", store: "credentials", required: true };
const detail = (s: ReturnType<typeof summary>, instances: unknown) => ({
  ...s, api_docs: null, connect_note: null, label_hint: null,
  connect: [{ id: "token", kind: "fields", label: null, description: null, recommended: false, fields: [field], scopes: [], default_scopes: [], access_levels: [], probe_scopes: [], client_key: null, callback: null, audience: "person", scope_presets: { read: [], write: [] }, show_context: [] }],
  tools: [], hosts: [], agent_instructions: null, credentials: [], metadata: [], oauth_clients: [], instances,
});

const GH = summary("github", "GitHub", fam("github.com", false), 2);
const GHES = summary("github-ghes", "GitHub Enterprise Server", fam("Enterprise Server", true), 1);
const SOLO = summary("solo", "Solo", null, 0);

interface Call { method: string; path: string; body: unknown }
function stub(extra: Record<string, (c: Call) => Response> = {}) {
  const calls: Call[] = [];
  const routes: Record<string, (c: Call) => Response> = {
    "GET /v1/apps": () => json(200, { items: [GHES, SOLO, GH] }),
    "GET /v1/apps/github": () => json(200, detail(GH, null)),
    "GET /v1/apps/github-ghes": () => json(200, detail(GHES, { label: "GHES host", default_host: null })),
    "GET /v1/apps/github-ghes/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/apps/github/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/me": () => json(200, { user: { display_name: "A", email: "a@x.io", is_instance_admin: false } }),
    "GET /v1/resolve": () => json(200, { org: { id: "o1", name: "Acme", slug: "acme" }, workspace: { id: "w1", name: "Main", slug: "main" } }),
    "GET /v1/orgs/o1/app-instances": () =>
      json(200, {
        items: [{ id: "i1", org_id: "o1", package_id: "github", app_id: "github-ghes", host: "ghe.acme.example", base_path: null, base_url: "https://ghe.acme.example", label: "Acme GHE", status: "active", created_by: null, created_at: "", updated_at: "", connections: 1, builtin: false }],
        next_cursor: null,
      }),
    "GET /v1/workspaces/w1/oauth-clients": () => json(200, { items: [], next_cursor: null }),
    ...extra,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string) => {
      // `useMe` calls fetch with a plain path; the generated client passes a Request.
      const req = typeof input === "string" ? null : input;
      const url = new URL(req ? req.url : input as string, "http://localhost");
      const method = req ? req.method : "GET";
      const c: Call = { method, path: url.pathname, body: method === "GET" || !req ? null : await req.json().catch(() => null) };
      calls.push(c);
      const h = routes[`${c.method} ${c.path}`];
      return h ? h(c) : json(404, { error: { code: "not_found", message: "x" } });
    }),
  );
  return calls;
}

describe("app names", () => {
  it("names a family member 'family · variant' and an app without one as it is", () => {
    expect(appDisplayName(GH)).toBe("GitHub · github.com");
    expect(appDisplayName(GHES)).toBe("GitHub · Enterprise Server");
    expect(appDisplayName(SOLO)).toBe("Solo");
    expect(productName(GHES)).toBe("GitHub");
    expect(productKey(GH)).toBe(productKey(GHES));
    expect(productKey(SOLO)).not.toBe(productKey(GH));
  });

  // Every list, picker and dialog that names an app goes through the shared helper (replace ad-hoc `app.name`).
  it("every app-naming surface imports the shared helper", () => {
    for (const f of [
      "components/accounts/free-apps.tsx",
      "components/agents/grant-editor.tsx",
      "components/credentials/give-access.tsx",
      "components/apps/org-apps-list.tsx",
      "components/apps/instances-view.tsx",
      "components/apps/oauth-clients-panel.tsx",
      "components/apps/apps-tab.tsx",
      "components/apps/connect-dialog.tsx",
    ]) {
      const src = readFileSync(`${__dirname}/../src/${f}`, "utf8");
      expect(src, f).toMatch(/@\/lib\/api\/app-name/);
    }
  });
});

describe("catalogue and connect window", () => {
  it("shows one tile per product: GitHub (both deployments' connections) and Solo", async () => {
    stub();
    wrap(<AppsTab workspaceId="w1" orgId="o1" canOwn />);
    await screen.findAllByRole("list", { name: /^Apps: / });
    // (workspace-9) The tiles sit in groups (Connected, Ready to connect…); together there is one per product.
    const lists = screen.getAllByRole("list", { name: /^Apps: / });
    const list = screen.getByRole("list", { name: "Apps: Connected" });
    const tiles = lists.flatMap((l) => within(l).getAllByRole("listitem"));
    expect(tiles).toHaveLength(2);
    expect(within(list).getByText("GitHub")).toBeInTheDocument();
    expect(within(list).queryByText("GitHub Enterprise Server")).toBeNull();
    // (P5-POLISH-7) A status line instead of "3 connections", and the button that follows it.
    expect(within(list).getByText("Connected · 3 accounts")).toBeInTheDocument();
    expect(within(list).queryByText(/connections/)).toBeNull();
    expect(within(list).getByRole("button", { name: "Connect another GitHub" })).toBeInTheDocument();
  });

  const step = (onPick: (w: { appId: string; instanceId?: string }) => void = () => {}) =>
    wrap(<DeploymentStep family={{ id: "github", name: "GitHub", variant_label: "github.com", instance_bound: false }} siblings={[GH, GHES] as never} orgId="o1" onPick={onPick} />);

  it("asks where the product runs and hands over the chosen server's app with its instance", async () => {
    stub();
    const onPick = vi.fn();
    step(onPick);
    const where = await screen.findByRole("radiogroup", { name: "Where does your GitHub run?" });
    await waitFor(() => expect(within(where).getAllByRole("radio")).toHaveLength(2));
    const labels = within(where).getAllByRole("radio").map((r) => r.closest("label")?.textContent ?? "");
    expect(labels[0]).toContain("github.com");
    expect(labels[1]).toContain("Enterprise Server · Acme GHE");
    expect(labels[1]).toContain("https://ghe.acme.example");
    fireEvent.click(within(where).getAllByRole("radio")[1]!);
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onPick).toHaveBeenCalledWith({ appId: "github-ghes", instanceId: "i1" });
  });

  it("the hosted deployment is picked without an instance", async () => {
    stub();
    const onPick = vi.fn();
    step(onPick);
    const where = await screen.findByRole("radiogroup", { name: "Where does your GitHub run?" });
    fireEvent.click(within(where).getAllByRole("radio")[0]!);
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onPick).toHaveBeenCalledWith({ appId: "github" });
  });

  it("without a registered server it says so (an instance admin is pointed at Self-hosted apps)", async () => {
    stub({
      "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/me": () => json(200, { user: { display_name: "A", email: "a@x.io", is_instance_admin: true } }),
    });
    step();
    expect(await screen.findByText(/No self-hosted server is registered yet/)).toBeInTheDocument();
    expect(await screen.findByRole("link", { name: /Register a server/ })).toHaveAttribute("href", "/admin/apps?tab=self-hosted");
  });
});

// FIX-202 (owner 2026-11-21): a family is ready when any variant is usable; a registered self-hosted server can be connected.
const jfam = (variant_label: string, instance_bound: boolean) => ({ id: "jira", name: "Jira", variant_label, instance_bound });
const jira = (app_id: string, name: string, family: ReturnType<typeof jfam>, readiness: unknown) => ({
  ...summary(app_id, name, family as never, 0),
  package_id: "jira",
  readiness,
});
const NEEDS = { state: "needs_admin", needs: ["oauth_client"], who_can: null, requested: null };
const OK = { state: "ready", needs: [], who_can: null, requested: null };
const JCLOUD = jira("jira", "Jira Cloud", jfam("Cloud", false), NEEDS);
const JDC = jira("jira-dc", "Jira Data Center", jfam("Data Center", true), OK);
const inst = (over: Record<string, unknown> = {}) => ({
  id: "ji1", org_id: "o1", package_id: "jira", app_id: "jira-dc", host: "its.cern.ch", base_path: "/jira", base_url: "https://its.cern.ch/jira",
  label: "CERN Jira", status: "active", created_by: null, created_at: "", updated_at: "", connections: 0, builtin: false, ...over,
});

describe("usableChoices (FIX-202)", () => {
  it("hosted needs an admin and a server is registered: ready, one usable choice", () => {
    expect(usableChoices([JCLOUD, JDC] as never, [inst()])).toEqual([{ appId: "jira-dc", instanceId: "ji1" }]);
    expect(isProductReady([JCLOUD, JDC] as never, [inst()])).toBe(true);
  });
  it("hosted needs an admin and no server: needs an admin", () => {
    expect(usableChoices([JCLOUD, JDC] as never, [])).toEqual([]);
    expect(isProductReady([JCLOUD, JDC] as never, [])).toBe(false);
  });
  it("both usable: two choices, the hosted one first", () => {
    const cloud = { ...JCLOUD, readiness: OK };
    expect(usableChoices([cloud, JDC] as never, [inst()])).toEqual([{ appId: "jira" }, { appId: "jira-dc", instanceId: "ji1" }]);
  });
  it("an inactive or built-in registration does not count", () => {
    expect(usableChoices([JCLOUD, JDC] as never, [inst({ status: "disabled" })])).toEqual([]);
    expect(usableChoices([JCLOUD, JDC] as never, [inst({ builtin: true })])).toEqual([]);
  });
});

describe("a registered self-hosted variant is connectable (FIX-202)", () => {
  const jiraRoutes = (cloud: unknown = JCLOUD) => ({
    "GET /v1/apps": () => json(200, { items: [cloud, JDC] }),
    "GET /v1/apps/jira": () => json(200, detail(cloud as never, null)),
    "GET /v1/apps/jira-dc": () => json(200, detail(JDC as never, { label: "Jira host", default_host: null })),
    "GET /v1/apps/jira-dc/connect-plan": () =>
      json(200, { app_id: "jira-dc", available: true, available_who_can: null, suggested_method: "token", methods: [{ id: "token", kind: "fields", label: "token", description: null, recommended: false, audience: "person", needs: [], ready: true, scope_presets: { read: [], write: [] } }] }),
    "GET /v1/workspaces/w1/accounts": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Daneel", handle: "daneel", kind: "hosted" }], next_cursor: null }),
    "GET /v1/apps/jira/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/apps/jira-dc/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/orgs/o1/app-instances": () => json(200, { items: [inst()], next_cursor: null }),
  });

  it("App step: Jira is Ready (1) with no Needs-an-admin entry, and opens the server directly", async () => {
    stub(jiraRoutes());
    const onPick = vi.fn();
    wrap(<AppStep workspaceId="w1" orgId="o1" onPick={onPick} />);
    expect(await screen.findByRole("tab", { name: "Ready · 1" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Needs an admin · 0" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Jira" }));
    expect(onPick).toHaveBeenCalledWith({ kind: "app", appId: "jira-dc", instanceId: "ji1" });
  });

  it("App step: with no server registered Jira needs an admin", async () => {
    stub({ ...jiraRoutes(), "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }) });
    wrap(<AppStep workspaceId="w1" orgId="o1" onPick={() => {}} />);
    expect(await screen.findByRole("tab", { name: "Needs an admin · 1" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Ready · 0" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Needs an admin · 1" }));
    expect(await screen.findByText("An admin needs to set up Jira sign-in")).toBeInTheDocument();
  });

  it("App step: both usable asks where Jira runs, preselected on the first usable option", async () => {
    stub(jiraRoutes({ ...JCLOUD, readiness: OK }));
    const onPick = vi.fn();
    wrap(<AppStep workspaceId="w1" orgId="o1" onPick={onPick} />);
    fireEvent.click(await screen.findByRole("button", { name: "Jira" }));
    const where = await screen.findByRole("radiogroup", { name: "Where does your Jira run?" });
    await waitFor(() => expect(within(where).getAllByRole("radio")).toHaveLength(2));
    expect(within(where).getAllByRole("radio")[0]).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onPick).toHaveBeenCalledWith({ kind: "app", appId: "jira" });
  });

  it("Deployment step: the unusable Cloud option shows its reason and the server is preselected", async () => {
    stub(jiraRoutes());
    const onPick = vi.fn();
    wrap(<DeploymentStep family={{ id: "jira", name: "Jira", variant_label: "Cloud", instance_bound: false }} siblings={[JCLOUD, JDC] as never} orgId="o1" onPick={onPick} />);
    const where = await screen.findByRole("radiogroup", { name: "Where does your Jira run?" });
    await waitFor(() => expect(within(where).getAllByRole("radio")).toHaveLength(2));
    const [cloud, server] = within(where).getAllByRole("radio");
    expect(cloud!.closest("label")?.textContent).toContain("An admin needs to set up Jira sign-in");
    expect(cloud).not.toBeChecked();
    expect(server).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onPick).toHaveBeenCalledWith({ appId: "jira-dc", instanceId: "ji1" });
  });

  it("Apps tab: the Jira card describes the Data Center app when only that one is usable, not Cloud", async () => {
    const cloud = { ...JCLOUD, summary: "Issues, projects and comments in Jira Cloud." };
    const server = { ...JDC, summary: "Issues, projects and comments on your Jira Data Center." };
    stub({ ...jiraRoutes(), "GET /v1/apps": () => json(200, { items: [cloud, server] }) });
    wrap(<AppsTab workspaceId="w1" orgId="o1" canOwn />);
    const list = await screen.findByRole("list", { name: "Apps: Ready to connect" });
    expect(await within(list).findByText("Issues, projects and comments on your Jira Data Center.")).toBeInTheDocument();
    expect(within(list).queryByText("Issues, projects and comments in Jira Cloud.")).toBeNull();
  });

  it("Apps tab: Connect on the Jira card opens the Data Center sign-in with the server, not Cloud", async () => {
    // jsdom has no `matchMedia`; the journey's frame asks whether the viewport is wide.
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: true, media: query, addEventListener: () => {}, removeEventListener: () => {} }));
    stub(jiraRoutes());
    wrap(<AppsTab workspaceId="w1" orgId="o1" canOwn />);
    const list = await screen.findByRole("list", { name: "Apps: Ready to connect" });
    fireEvent.click(await within(list).findByRole("button", { name: "Connect Jira" }));
    // No question and no Cloud step: the journey goes on with the Data Center app (its name is on the Agent step).
    expect(await screen.findByText(/Which agents can use Jira · Data Center\?/)).toBeInTheDocument();
    expect(screen.queryByText(/Jira · Cloud/)).toBeNull();
    expect(screen.queryByRole("radiogroup", { name: "Where does your Jira run?" })).toBeNull();
  });

  it("Sign in step: with no server registered, 'Use a self-hosted Jira' links to the Data Center app; with one it says 'Use Jira Data Center'", async () => {
    const cloudReady = { ...JCLOUD, readiness: OK };
    const none = { "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }) };
    stub({ ...jiraRoutes(cloudReady), ...none });
    const plan = { suggested_method: "token", methods: [{ id: "token", kind: "fields", ready: true, audience: "member", who_can: null }], available_who_can: null };
    const props = { app: detail(cloudReady as never, null) as never, plan: plan as never, workspaceId: "w1", orgId: "o1", level: null, agentIds: [], hasWriteTools: false, onResult: () => {} };
    const first = wrap(<SignInStep {...props} />);
    const more = await screen.findByText("More options");
    fireEvent.click(more);
    expect(await screen.findByRole("link", { name: "Use a self-hosted Jira" })).toHaveAttribute("href", "/o/acme/w/main/apps/jira-dc");
    first.unmount();
    stub(jiraRoutes(cloudReady));
    const onChoose = vi.fn();
    wrap(<SignInStep {...props} onChooseDeployment={onChoose} />);
    fireEvent.click(await screen.findByText("More options"));
    expect(screen.queryByRole("link", { name: "Use a self-hosted Jira" })).toBeNull();
    fireEvent.click(await screen.findByRole("button", { name: "Use Jira Data Center" }));
    expect(onChoose).toHaveBeenCalled();
  });
});
