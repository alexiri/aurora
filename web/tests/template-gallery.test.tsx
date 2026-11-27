import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CreateAgentDialog } from "@/components/agents/create-agent-dialog";
import { notReadyWords } from "@/components/templates/words";
import { runsOnItsOwn, TemplateCardView, TemplateDetails, TemplateGalleryDialog } from "@/components/templates/gallery";
import { wrap } from "./support/render";
import { json, type Handler } from "./support/api";

// P6-TPL-4 Part B1, C-1116 (D-458): **More templates…** in the New agent dialog's Start from row opens the gallery of
// cards (ready and not-ready); choosing a card opens its details first (the README rendered the C-1116 way, what it
// needs, what it runs on its own, helper files by name, the version) before **Use this template**.

const push = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: vi.fn(), push }), useParams: () => ({ org: "acme", ws: "ops" }) }));

const README =
  "# Schedule manager\n\nWhat it does.\n\n<img src=x onerror=alert(1)>\n\n[click](javascript:alert(1))\n\n![remote picture](https://evil.test/p.png)\n\n[docs](./other.md)\n";

const card = (over: Record<string, unknown> = {}) => ({
  template_id: "schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work",
  readme: README, version: "1.0.0", has_avatar: false, source_name: "Acme templates",
  apps: [{ app_ids: ["github"], required: true, available: true, skills: [] }],
  skill_refs: [] as { index: number; kind: string; name: string; required: boolean; available: boolean; repo?: string | null }[],
  has_helpers: true,
  automations: [{ kind: "schedule", name: "Morning triage" }, { kind: "webhook", name: "Alerts" }],
  ready: true,
  ...over,
});

const plan = {
  ...card(),
  content_sha256: "c".repeat(64),
  content: {
    name: "Schedule manager", summary: "Triage and schedule the team's work", readme: README, effort: null, min_core: null,
    agent: { name: "Daneel", may_message_first: false }, instructions: "Triage carefully.", inputs: [], apps: [], skills: [], skill_refs: [],
    helpers: [{ path: "triage.py", size: 120, executable: true, sha256: "a".repeat(64), text: "print(1)" }],
    memories: [], automations: [], has_avatar: false,
  },
  entries: [],
  skill_refs: [],
};

const appSummary = { app_id: "github", name: "GitHub", family: null, description: "", available: true, connection_count: 0, credential_free: true, has_dark_icon: true, package_id: "github", package_version: "1.0.0", send_only: false, app_restrictions: {} };

function stubFetch(items: unknown[], extra: Record<string, Handler> = {}) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const p = decodeURIComponent(url.pathname);
    const routes: Record<string, Handler> = {
      "GET /v1/workspaces/w1/agent-templates": () => json(200, { items }),
      "GET /v1/workspaces/w1/agent-templates/schedule-manager": () => json(200, plan),
      "GET /v1/apps": () => json(200, { items: [appSummary] }),
      ...extra,
    };
    const h = routes[`${req.method} ${p}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("More templates… (C-1116)", () => {
  // (FIX-245) The gallery is always reachable from the Agents page, so an empty one explains itself.
  describe("empty gallery", () => {
    function stubEmpty(me: Record<string, unknown>) {
      const fn = vi.fn(async (input: Request | string | URL) => {
        const method = input instanceof Request ? input.method : "GET";
        const p = new URL(input instanceof Request ? input.url : String(input), "http://localhost").pathname;
        const routes: Record<string, Handler> = {
          "GET /v1/workspaces/w1/agent-templates": () => json(200, { items: [] }),
          "GET /v1/me": () => json(200, me),
        };
        const h = routes[`${method} ${p}`];
        return h ? h(input as Request) : json(404, { error: { code: "not_found", message: "nf" } });
      });
      vi.stubGlobal("fetch", fn);
    }

    it("a member reads that the administrators have not approved any yet, with no dead end", async () => {
      stubEmpty({ user: { display_name: "Pat Person", email: "p@x.io" } });
      wrap(<TemplateGalleryDialog open onOpenChange={() => undefined} wsId="w1" onUse={() => undefined} />);
      const dlg = await screen.findByRole("dialog", { name: "More templates" });
      expect(await within(dlg).findByText("Your administrators have not approved any agent templates yet.")).toBeTruthy();
      expect(within(dlg).queryByRole("link")).toBeNull();
    });

    it("an administrator is told to review the waiting ones and sent to the Agent templates tab", async () => {
      stubEmpty({ user: { display_name: "Ada Admin", email: "a@x.io", is_instance_admin: true } });
      wrap(<TemplateGalleryDialog open onOpenChange={() => undefined} wsId="w1" onUse={() => undefined} />);
      const dlg = await screen.findByRole("dialog", { name: "More templates" });
      expect(await within(dlg).findByText("No templates are approved yet. Review the ones waiting.")).toBeTruthy();
      const link = within(dlg).getByRole("link", { name: "Open Agent templates" });
      expect(link.getAttribute("href")).toBe("/admin/internet-access?tab=templates");
    });
  });

  it("appears in the Start from row when the workspace has templates, and opens the gallery", async () => {
    stubFetch([card()]);
    wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    fireEvent.click(await screen.findByRole("button", { name: "More templates…" }));
    const dlg = await screen.findByRole("dialog", { name: "More templates" });
    expect(within(dlg).getByText("Schedule manager")).toBeTruthy();
  });

  it("does not appear when the workspace has no templates", async () => {
    stubFetch([]);
    wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    await screen.findByLabelText("Name");
    expect(screen.queryByRole("button", { name: "More templates…" })).toBeNull();
  });

  it("goes to the four-step page from Use this template", async () => {
    stubFetch([card()]);
    wrap(<CreateAgentDialog wsId="w1" />);
    fireEvent.click(screen.getByRole("button", { name: "New agent" }));
    fireEvent.click(await screen.findByRole("button", { name: "More templates…" }));
    fireEvent.click(await screen.findByRole("button", { name: /Schedule manager/ }));
    // The Use button waits for the plan to load before it is enabled.
    const use = await screen.findByRole("button", { name: "Use this template" });
    await waitFor(() => expect(use).not.toBeDisabled());
    fireEvent.click(use);
    await waitFor(() => expect(push).toHaveBeenCalledWith("/o/acme/w/ops/agents/new/schedule-manager"));
  });
});

describe("the gallery card (C-1116)", () => {
  const appName = (id: string) => (id === "github" ? "GitHub" : id);

  it("shows avatar, name, summary, app icons and what it runs on its own", () => {
    const { container } = wrap(<ul><TemplateCardView card={card() as never} wsId="w1" appName={appName} onOpen={() => undefined} /></ul>);
    expect(screen.getByText("Schedule manager")).toBeTruthy();
    expect(screen.getByText("Version 1.0.0")).toBeTruthy();
    expect(screen.getByText("Triage and schedule the team's work")).toBeTruthy();
    expect(screen.getByText("Schedule Morning triage")).toBeTruthy();
    expect(screen.getByText("Webhook Alerts, created paused")).toBeTruthy();
    expect(screen.getByLabelText("Apps it needs")).toBeTruthy();
    // No avatar on the card: the initial tile renders (the only img is the app icon's).
    expect(container.querySelectorAll("img").length).toBe(1);
    expect(container.querySelector("img[src*='avatar']")).toBeNull();
    expect(screen.getByText("S")).toBeTruthy();
  });

  it("says what is missing: an app that is not available, and a library skill", () => {
    const notReady = card({
      ready: false,
      apps: [{ app_ids: ["jira"], required: true, available: false, skills: [] }],
      skill_refs: [
        { index: 0, kind: "library", name: "incident-writeup", required: true, available: false, repo: "git.example/acme/skills" },
        { index: 1, kind: "organization", name: "triage-rules", required: false, available: false },
      ],
    });
    wrap(<ul><TemplateCardView card={notReady as never} wsId="w1" appName={appName} onOpen={() => undefined} /></ul>);
    expect(screen.getByText("Needs jira, which isn't available here.")).toBeTruthy();
    // The card names the library's repository (C-1116).
    expect(screen.getByText("Needs the skill library git.example/acme/skills.")).toBeTruthy();
    expect(screen.queryByText(/triage-rules/)).toBeNull(); // an optional reference is not "missing"
  });

  it("notReadyWords is empty for a ready card and names every required gap otherwise", () => {
    expect(notReadyWords(card() as never, appName)).toEqual([]);
    const both = card({
      ready: false,
      apps: [{ app_ids: ["jira"], required: true, available: false, skills: [] }],
      skill_refs: [{ index: 0, kind: "library", name: "x", required: true, available: false, repo: "git.example/acme/skills" }],
    });
    expect(notReadyWords(both as never, appName)).toEqual(["Needs jira, which isn't available here.", "Needs the skill library git.example/acme/skills."]);
  });

  it("runsOnItsOwn names the schedules and marks the webhooks paused", () => {
    expect(runsOnItsOwn([{ kind: "schedule", name: "a" }, { kind: "webhook", name: "b" }])).toEqual(["Schedule a", "Webhook b, created paused"]);
    expect(runsOnItsOwn([])).toEqual([]);
  });
});

describe("the details view (D-458)", () => {
  function details(cardOver: Record<string, unknown> = {}, extra: Record<string, Handler> = {}) {
    stubFetch([card()], extra);
    return wrap(<TemplateDetails card={card(cardOver) as never} wsId="w1" onBack={() => undefined} onUse={() => undefined} />);
  }

  it("renders the README the C-1116 way: nothing active, nothing loaded, headings shifted", () => {
    details();
    const readme = screen.getByTestId("template-readme");
    expect(readme.textContent).toContain("<img src=x onerror=alert(1)>");
    expect(readme.querySelector("img")).toBeNull();
    expect(readme.querySelector("a[href^='javascript:']")).toBeNull();
    expect(screen.getByText("remote picture")).toBeTruthy();
    expect(screen.getByText("docs")).toBeTruthy();
    // The README's `# Schedule manager` is a level-3 heading inside the readme; the details header is another h3.
    expect(within(readme).getByRole("heading", { level: 3, name: "Schedule manager" })).toBeTruthy();
  });

  it("shows what it needs, what it runs on its own, helper files by name, the version and the source", async () => {
    details();
    // The app name comes from the catalogue fetch, so it arrives after the first render.
    expect(await screen.findByText("GitHub — required")).toBeTruthy();
    expect(screen.getByText("Schedule Morning triage")).toBeTruthy();
    expect(screen.getByText("Webhook Alerts, created paused")).toBeTruthy();
    expect(await screen.findByText("triage.py")).toBeTruthy();
    expect(screen.getAllByText("Version 1.0.0 · from Acme templates").length).toBeGreaterThan(0);
  });

  it("disables Use this template while the template is not ready, and shows why", () => {
    details({ ready: false, apps: [{ app_ids: ["jira"], required: true, available: false, skills: [] }] });
    const use = screen.getByRole("button", { name: "Use this template" }) as HTMLButtonElement;
    expect(use.disabled).toBe(true);
    expect(screen.getByText("Needs jira, which isn't available here.")).toBeTruthy();
  });
});
