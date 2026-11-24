import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { NewAgentFromTemplateView } from "@/components/templates/new-agent/page-view";
import { json, type Handler } from "./support/api";
import { wrap } from "./support/render";

// P6-TPL-4 Part B1, C-1116: **Use this template** opens the four-step page — the fold **About this template** (the
// README, closed, inert) above the steps. About asks name, handle and the template's inputs; Accounts asks each
// entry's app, its account (existing, new through the account-only journey, or skipped) and the access; Automations
// shows the schedules and the paused webhooks; Review shows what the agent can do, the parts as data and the library
// skills; **Create** sends it, shows each webhook secret once, then opens the agent's chat.

const push = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace: vi.fn(), push }),
  useParams: () => ({ org: "acme", ws: "main", template_id: "schedule-manager" }),
}));

const README =
  "# Schedule manager\n\nWhat it does.\n\n<img src=x onerror=alert(1)>\n\n[click](javascript:alert(1))\n\n![remote picture](https://evil.test/p.png)\n\n[docs](./other.md)\n";

const account = (id: string, name: string, over: Record<string, unknown> = {}) => ({
  ref: { type: "connection", id },
  kind: "app",
  name,
  scope: "workspace",
  levels: ["read", "ask", "act"],
  app: { app_id: "github", name: "GitHub", custom_only: false },
  health: { state: "not_used" },
  agents: [],
  connected_by: null,
  can: { give_access: true, change_access: true, manage: true },
  ...over,
});

const GITHUB = account("c1", "GitHub · bob-builder");
const LINEAR = { ...account("c2", "Linear · acme"), app: { app_id: "linear", name: "Linear", custom_only: false } };

/** A library commit head; Review shows its first seven characters. */
const SHA = "abcdef1234567890abcdef1234567890abcdef12";

const library = () => ({ id: "lib1", label: "Team skills", repo: "github.com/acme/skills", path: "", ref: null, commit_sha: SHA, scope: "organization" });

const choice = (over: Record<string, unknown> = {}) => ({
  app_id: "github", app_name: "GitHub", available: true, accounts: [GITHUB], level: "ask", level_note: null,
  custom: null, unavailable_reason: null,
  ...over,
});

const plan = {
  template_id: "schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work",
  readme: README, version: "1.0.0", has_avatar: false, has_helpers: true, ready: true, content_sha256: "c".repeat(64),
  apps: [{ app_ids: ["github"], required: true, available: true, skills: [] }],
  automations: [],
  content: {
    name: "Schedule manager", summary: "Triage and schedule the team's work", readme: README, effort: null, min_core: null,
    agent: { name: "Daneel", may_message_first: false }, instructions: "Triage carefully.",
    inputs: [
      { id: "repo", label: "Repository", help: "owner/name", kind: "text", required: true, pattern: null, max_length: 100, options: null, default: null },
      { id: "region", label: "Region", help: null, kind: "choice", required: true, pattern: null, max_length: null, options: ["eu", "us"], default: "eu" },
    ],
    apps: [],
    skills: [{ name: "Triage", use_it_when: "The queue is long.", body: "Read the queue. Sort it.", files: [] }],
    skill_refs: [],
    helpers: [{ path: "triage.py", executable: false, sha256: "f".repeat(64), size: 12, text: "print('hi')" }],
    memories: [{ key: "tone", locked: false, content: "Be brief." }],
    automations: [
      { kind: "schedule", name: "Morning triage", prompt: "Triage the queue.", cron: "0 9 * * 1-5" },
      { kind: "webhook", name: "Alerts", instructions: "Post alerts to the room.", verification: "signed" },
    ],
    has_avatar: false,
  },
  entries: [
    { index: 0, required: true, why: "It triages pull requests.", choices: [choice()] },
    { index: 1, required: false, why: "It files cards.", choices: [choice({ app_id: "linear", app_name: "Linear", accounts: [LINEAR], level: "read" })] },
  ],
  skill_refs: [
    { index: 0, kind: "library", name: "Triage", required: true, available: true, why: "It triages the queue.", library: library(), content: { use_it_when: "The queue is long.", body: "Read the queue.", files: [] } },
    { index: 1, kind: "library", name: "Sorting", required: false, available: true, why: "", library: library(), content: { use_it_when: "The queue needs order.", body: "Sort it newest first.", files: [] } },
    { index: 2, kind: "library", name: "Reports", required: false, available: false, why: "", library: null, unavailable_reason: "library_unreadable", content: null },
    { index: 3, kind: "organization", name: "House style", required: true, available: true, why: "", library: null, content: null },
  ],
};

const DETAIL = {
  app_id: "github", name: "GitHub", package_id: "github", package_version: "2.0.0", description: "d", available: true, connection_count: 0,
  has_dark_icon: false, category: "development", api_docs: null, connect_note: null, label_hint: null, credential_free: false,
  app_restrictions: { custom_only: false, block_untooled: false },
  connect: [
    {
      id: "token", kind: "fields", label: "Token", description: null, recommended: true, audience: "person", needs: [], ready: true,
      fields: [{ name: "access_token", label: "Token", type: "secret", store: "credentials", required: true }],
      scopes: [], default_scopes: [], access_levels: [], probe_scopes: [], client_key: null, callback: null,
      scope_presets: { read: [], write: [] }, permission_hints: [], show_context: [],
    },
    {
      id: "oauth", kind: "oauth2", label: "Sign in", description: null, recommended: false, audience: "person", needs: [], ready: true, fields: [],
      scopes: [], default_scopes: [], access_levels: [], probe_scopes: [], client_key: "gh", callback: "query",
      scope_presets: { read: [], write: [] }, permission_hints: [], show_context: [],
    },
  ],
  first_prompt: null, tools: [], hosts: [], agent_instructions: null, credentials: [], metadata: [], oauth_clients: [], instances: null,
};

function stubFetch(extra: Record<string, Handler> = {}) {
  let planGets = 0;
  const routes: Record<string, Handler> = {
    "GET /v1/resolve": () => json(200, { org: { id: "o1", name: "Acme" }, workspace: { id: "w1", name: "Main" } }),
    "GET /v1/workspaces/w1/agent-templates/schedule-manager": () => {
      planGets += 1;
      return json(200, plan);
    },
    "GET /v1/orgs/o1": () => json(200, { id: "o1", name: "Acme" }),
    "GET /v1/workspaces/w1": () => json(200, { id: "w1", name: "Main", timezone: "Europe/Zurich" }),
    "GET /v1/apps": () => json(200, { items: [{ ...DETAIL, connection_count: 0 }], next_cursor: null }),
    "GET /v1/apps/github": () => json(200, DETAIL),
    "GET /v1/apps/github/guide": () => json(404, { error: { code: "not_found", message: "x" } }),
    "GET /v1/apps/github/connect-plan": () => json(200, { app_id: "github", available: true, suggested_method: "token", methods: DETAIL.connect }),
    "GET /v1/workspaces/w1/oauth-clients": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/workspaces/w1/accounts": () => json(200, { items: [], next_cursor: null }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Daneel", handle: "daneel", kind: "hosted" }], next_cursor: null }),
    "GET /v1/orgs/o1/app-instances": () => json(200, { items: [], next_cursor: null }),
    ...extra,
  };
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${decodeURIComponent(url.pathname)}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return { fn, planGets: () => planGets };
}

beforeEach(() => {
  push.mockClear();
  // jsdom has no `matchMedia`; the journey's sheet asks whether the viewport is wide.
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query.includes("min-width"), media: query, onchange: null, addEventListener: () => undefined, removeEventListener: () => undefined,
    addListener: () => undefined, removeListener: () => undefined, dispatchEvent: () => false,
  }));
  vi.stubGlobal("BroadcastChannel", class {
    static open = new Set<never>();
    onmessage: ((e: { data: unknown }) => void) | null = null;
    postMessage() {}
    close() {}
  });
  vi.stubGlobal("open", vi.fn(() => ({ closed: false, opener: window, location: { replace: vi.fn() }, focus: vi.fn(), close: vi.fn() })));
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const openPlan = (extra: Record<string, Handler> = {}) => {
  const stub = stubFetch(extra);
  wrap(<NewAgentFromTemplateView />);
  return stub;
};

/** Fills About with valid answers (leaving `over` to override) and goes to Accounts. */
const gotoAccounts = async (name = "Rex", repo = "acme/town", region: string | null = null) => {
  await screen.findByLabelText("Repository");
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: name } });
  if (repo !== null) fireEvent.change(screen.getByLabelText("Repository"), { target: { value: repo } });
  if (region !== null) fireEvent.change(screen.getByLabelText("Region"), { target: { value: region } });
  fireEvent.click(screen.getByRole("button", { name: "Next" }));
};

/** Accounts → Automations → Review. */
const gotoReview = async () => {
  await gotoAccounts();
  fireEvent.click(await screen.findByRole("button", { name: "Next" }));
  fireEvent.click(await screen.findByRole("button", { name: "Next" }));
};

/** The stepper's items (scoped: the breadcrumb's list items are on the page too). */
const stepItems = () => within(screen.getByRole("list", { name: "New agent steps" })).getAllByRole("listitem");

describe("the four-step page (C-1116)", () => {
  it("the fold holds the inert README above the steps, and About asks name, handle and the inputs", async () => {
    openPlan();
    await screen.findByLabelText("Repository");
    const fold = screen.getByTestId("template-fold") as HTMLDetailsElement;
    expect(fold.open).toBe(false);
    const readme = within(fold).getByTestId("template-readme");
    expect(readme.textContent).toContain("<img src=x onerror=alert(1)>");
    expect(readme.querySelector("img")).toBeNull();
    expect(readme.querySelector("a[href^='javascript:']")).toBeNull();
    const steps = screen.getByRole("list", { name: "New agent steps" });
    const items = within(steps).getAllByRole("listitem");
    expect(items.map((l) => l.textContent)).toEqual(["About", "Accounts", "Automations", "Review"]);
    expect(items[0]).toHaveAttribute("aria-current", "step");
    expect(items[1].getAttribute("aria-current")).toBeNull();
    expect(screen.getByLabelText("Name")).toBeTruthy();
    expect(screen.getByLabelText("Handle (optional)")).toBeTruthy();
    expect(screen.getByLabelText("Repository")).toBeTruthy();
    // A choice input starts at the manifest's default.
    expect((screen.getByLabelText("Region") as HTMLSelectElement).value).toBe("eu");
    // Crumbs lead back to the agents page.
    expect(screen.getByRole("link", { name: "Agents" })).toBeTruthy();
  });

  it("Next refuses an empty name, a bad handle and an empty required input; then it goes on", async () => {
    openPlan();
    await screen.findByLabelText("Repository");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("Name must be 1 to 80 characters.")).toBeTruthy();
    expect(screen.getByText("This is needed.")).toBeTruthy(); // the required Repository input
    expect(screen.getAllByRole("listitem")[1]!.getAttribute("aria-current")).toBeNull();

    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Rex" } });
    fireEvent.change(screen.getByLabelText("Repository"), { target: { value: "acme/town" } });
    fireEvent.change(screen.getByLabelText("Handle (optional)"), { target: { value: "Rex!" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("Use lowercase letters, digits and dashes, starting with a letter (max 40).")).toBeTruthy();

    fireEvent.change(screen.getByLabelText("Handle (optional)"), { target: { value: "rex" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("It triages pull requests.")).toBeTruthy();
  });

  it("Accounts asks each entry: the first existing account is recommended, the access is preselected, optional entries can be skipped", async () => {
    openPlan();
    await gotoAccounts();
    await screen.findByText("It triages pull requests.");
    const githubEntry = screen.getByRole("region", { name: "It triages pull requests." });
    expect(within(githubEntry).getByRole("radio", { name: /Use GitHub · bob-builder/ })).toBeChecked();
    // Two badges: the first existing account, and the Read only card of the access (its plan level is ask).
    expect(within(githubEntry).getAllByText("Recommended").length).toBe(2);
    expect(within(githubEntry).queryByRole("radio", { name: /Skip GitHub/ })).toBeNull(); // a required entry cannot be skipped
    expect(within(githubEntry).getByRole("radio", { name: /Ask before changes/ })).toBeChecked(); // the plan's level
    expect(within(githubEntry).getByRole("radio", { name: /Read only/ })).toBeTruthy();

    const linearEntry = screen.getByRole("region", { name: "It files cards." });
    expect(within(linearEntry).getByRole("radio", { name: /Skip Linear/ })).toBeTruthy();
    fireEvent.click(within(linearEntry).getByRole("radio", { name: /Skip Linear/ }));
    expect(within(linearEntry).getByRole("radio", { name: /Skip Linear/ })).toBeChecked();
    // Leaving the access alone changes nothing: the level is sent only when the person changes it.
    expect(within(githubEntry).getByRole("radio", { name: /Read only/ })).toBeTruthy();
  });

  it("a custom_only account says the organization asks before each change, with the tool lists", async () => {
    const customPlan = {
      ...plan,
      entries: [
        {
          index: 0, required: true, why: "It triages pull requests.",
          choices: [choice({ level: "ask", level_note: "custom_only", custom: { allow_tools: ["read_issues"], ask_tools: ["create_issue"], dropped_tools: ["delete_issue"] } })],
        },
      ],
    };
    stubFetch({ "GET /v1/workspaces/w1/agent-templates/schedule-manager": () => json(200, customPlan) });
    wrap(<NewAgentFromTemplateView />);
    await gotoAccounts();
    await screen.findByText("It triages pull requests.");
    expect(screen.getByText("The organization limits this app: it asks you before each change.")).toBeTruthy();
    expect(screen.getByText(/Allowed without asking:/)).toHaveTextContent("read_issues");
    expect(screen.getByText(/Needs your approval:/)).toHaveTextContent("create_issue");
    expect(screen.getByText(/Not available here:/)).toHaveTextContent("delete_issue");
    expect(screen.queryByRole("radio", { name: /Ask before changes/ })).toBeNull(); // no access to choose
  });

  it("an app that is not available says so instead of asking for an account", async () => {
    const downPlan = {
      ...plan,
      entries: [{ index: 0, required: true, why: "It triages pull requests.", choices: [choice({ available: false, accounts: [], unavailable_reason: "not_installed" })] }],
    };
    stubFetch({ "GET /v1/workspaces/w1/agent-templates/schedule-manager": () => json(200, downPlan) });
    wrap(<NewAgentFromTemplateView />);
    await gotoAccounts();
    expect(await screen.findByText("Needs GitHub, which isn't installed here.")).toBeTruthy();
  });

  it("Connect a new account opens the account-only journey; closing it refetches the plan", async () => {
    const stub = openPlan();
    await gotoAccounts();
    const githubEntry = await screen.findByRole("region", { name: "It triages pull requests." });
    const before = stub.planGets();
    fireEvent.click(within(githubEntry).getByRole("radio", { name: /Connect a new GitHub account/ }));
    const journey = await screen.findByRole("dialog");
    expect(journey.getAttribute("data-testid")).toBe("ws-connect-journey");
    // Straight to Sign in: no Agent or Access step. "Token" names both the method choice and its field.
    expect((await screen.findAllByLabelText("Token")).length).toBeGreaterThanOrEqual(1);
    fireEvent.click(screen.getAllByRole("button", { name: "Close" })[0]!);
    await waitFor(() => expect(stub.planGets()).toBe(before + 1));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("Back keeps what was filled in", async () => {
    openPlan();
    await gotoAccounts();
    fireEvent.click(await screen.findByRole("button", { name: "Back" }));
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Rex");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("It triages pull requests.")).toBeTruthy();
  });

  it("the Automations step shows each schedule in words with the workspace's zone, and webhooks as created paused", async () => {
    openPlan();
    await gotoAccounts();
    fireEvent.click(await screen.findByRole("button", { name: "Next" }));
    const schedule = await screen.findByRole("switch", { name: "Morning triage" });
    expect(schedule).toBeChecked(); // the template's default: a schedule left on is created on
    expect(screen.getByText("Triage the queue.")).toBeTruthy();
    expect(screen.getByText("Every weekday at 09:00 · Zurich")).toBeTruthy();
    fireEvent.click(schedule);
    expect(schedule).not.toBeChecked();
    // A webhook is not chosen here: it is listed as data, created paused, with its verification words.
    expect(screen.getByText(/Created paused\. Post alerts to the room\./)).toHaveTextContent("Verified with a signature.");
    expect(screen.queryByRole("switch", { name: "Alerts" })).toBeNull();
    expect(screen.getByText("Webhooks are created paused; the agent turns one on when you give it the address.")).toBeTruthy();
  });

  it("Review shows what the agent can do, the parts as inert data and the library skills at their commit", async () => {
    openPlan();
    await gotoReview();
    await screen.findByTestId("review-step");
    // C-986's sentences over the planned accounts; the Internet access cannot be read here, so nothing about websites.
    expect(screen.getByText("Rex can read GitHub and Linear.")).toBeTruthy();
    expect(screen.getByText("It asks you before changing anything in GitHub.")).toBeTruthy();
    const textOf = (testId: string) => screen.getByTestId(testId).textContent ?? "";
    for (const testId of ["review-instructions", "review-skills", "review-helpers", "review-memories"]) {
      fireEvent.click(screen.getByTestId(testId).querySelector("summary")!);
    }
    expect(textOf("review-instructions")).toContain("Triage carefully.");
    expect(textOf("review-skills")).toContain("Read the queue. Sort it.");
    expect(textOf("review-helpers")).toContain("triage.py");
    expect(textOf("review-memories")).toContain("Be brief.");
    // The library skills: the commit head in seven characters, as data.
    const libraries = screen.getByRole("region", { name: "Skills from libraries" });
    expect(within(libraries).getByTestId("ref-0").textContent).toContain("From Team skills (github.com/acme/skills) at abcdef1");
    // An optional reference is switched off, which hides its content.
    const sorting = within(libraries).getByRole("switch", { name: "Add Sorting" });
    expect(sorting).toBeChecked();
    fireEvent.click(sorting);
    expect(within(libraries).getByTestId("ref-1").textContent).not.toContain("Sort it newest first.");
    // An unavailable one is not added, in the sentence that says why.
    expect(within(libraries).getByTestId("ref-2").textContent).toContain("Not added: needs the Reports skill, which isn't available here.");
    // An organization skill is not a library skill: it is not listed here.
    expect(within(libraries).queryByTestId("ref-3")).toBeNull();
  });

  it("Create sends the request built from the steps: accounts without unchanged levels, library skills, schedules", async () => {
    let body: Record<string, unknown> | null = null;
    const created = { agent: { id: "a2", name: "Rex", handle: "rex", kind: "hosted" }, grants: [], notes: [], skills: [], webhooks: [] };
    openPlan({
      "POST /v1/workspaces/w1/agents/from-template": async (req) => {
        body = await (req as Request).json();
        return json(201, created);
      },
    });
    await gotoAccounts("Rex", "acme/town", "us");
    fireEvent.click(within(await screen.findByRole("region", { name: "It files cards." })).getByRole("radio", { name: /Skip Linear/ }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(await screen.findByRole("switch", { name: "Morning triage" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(await screen.findByRole("button", { name: "Create" }));
    await waitFor(() => expect(body).toBeTruthy());
    expect(body).toEqual({
      template_id: "schedule-manager",
      version: "1.0.0",
      content_sha256: "c".repeat(64),
      name: "Rex",
      handle: undefined,
      inputs: { repo: "acme/town", region: "us" },
      // The skipped entry sends nothing; the access is sent only when it was changed (it was not).
      accounts: [{ entry: 0, app_id: "github", connection_id: "c1" }],
      // The available library references, required and optional; organization ones are never sent.
      skill_refs: [
        { index: 0, library_id: "lib1", commit_sha: SHA },
        { index: 1, library_id: "lib1", commit_sha: SHA },
      ],
      // Only the schedule is sent; webhooks are created paused without a choice.
      automations: [{ kind: "schedule", name: "Morning triage", enabled: false }],
    });
  });

  it("a webhook's secret is shown once, then the agent's chat opens", async () => {
    const created = {
      agent: { id: "a2", name: "Rex", handle: "rex", kind: "hosted" },
      grants: [], notes: [], skills: [],
      webhooks: [{ webhook: { id: "wh1", name: "Alerts" }, url: "/v1/workspaces/w1/webhooks/wh1", secret: "whsec_123" }],
    };
    openPlan({ "POST /v1/workspaces/w1/agents/from-template": () => json(201, created) });
    await gotoReview();
    fireEvent.click(await screen.findByRole("button", { name: "Create" }));
    expect((await screen.findByTestId("secret-key")).textContent).toBe("whsec_123");
    expect(screen.getAllByText("Webhook Alerts").length).toBeGreaterThan(0);
    expect(push).not.toHaveBeenCalled();
    // Closing without copying asks first.
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    expect(screen.getByText("You haven't copied it. Close anyway?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Close anyway" }));
    expect(push).toHaveBeenCalledWith("/o/acme/w/main/agents/rex");
    expect(screen.queryByTestId("secret-key")).toBeNull();
  });

  it("a 409 template_version_changed swaps in the plan the refusal carries and asks for a look", async () => {
    const nextPlan = { ...plan, version: "1.1.0", readme: "# Swapped in\n" };
    let body: Record<string, unknown> | null = null;
    openPlan({
      "POST /v1/workspaces/w1/agents/from-template": async (req) => {
        body = await (req as Request).json();
        return json(409, { error: { code: "template_version_changed", message: "The template moved on.", details: { plan: nextPlan } } });
      },
    });
    await gotoReview();
    fireEvent.click(await screen.findByRole("button", { name: "Create" }));
    expect(await screen.findByText("This template was updated. Check the changes, then create.")).toBeTruthy();
    // The plan the refusal carried is what the page now shows.
    expect(screen.getByTestId("template-fold").textContent).toContain("Swapped in");
    // Creating again sends the offered version the refusal carried.
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(body?.version).toBe("1.1.0"));
  });

  it("a 422 with field errors jumps to the step the field belongs to and says so on it", async () => {
    openPlan({
      "POST /v1/workspaces/w1/agents/from-template": () =>
        json(422, {
          error: {
            code: "template_inputs_invalid", message: "The answers don't fit.",
            details: { fields: [{ path: "/inputs/repo", message: "Write owner/name." }] },
          },
        }),
    });
    await gotoReview();
    fireEvent.click(await screen.findByRole("button", { name: "Create" }));
    expect(await screen.findByText("Write owner/name.")).toBeTruthy();
    expect(stepItems()[0]).toHaveAttribute("aria-current", "step");
    // What was filled in is kept.
    expect((screen.getByLabelText("Repository") as HTMLInputElement).value).toBe("acme/town");
  });

  it("a 422 about the accounts opens the Accounts step with the message; a plain refusal says so where you are", async () => {
    let calls = 0;
    openPlan({
      "POST /v1/workspaces/w1/agents/from-template": () => {
        calls += 1;
        return calls === 1
          ? json(422, { error: { code: "template_accounts_invalid", message: "x", details: { fields: [{ path: "/accounts", message: "That account is gone." }] } } })
          : json(403, { error: { code: "forbidden", message: "You cannot create agents here." } });
      },
    });
    await gotoReview();
    fireEvent.click(await screen.findByRole("button", { name: "Create" }));
    expect(await screen.findByText("That account is gone.")).toBeTruthy();
    expect(stepItems()[1]).toHaveAttribute("aria-current", "step");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(await screen.findByRole("button", { name: "Create" }));
    expect(await screen.findByText("You cannot create agents here.")).toBeTruthy(); // a danger Notice on the review step
  });
});
