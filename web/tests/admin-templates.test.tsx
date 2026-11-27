import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AdminTemplates } from "@/components/templates/admin-templates";
import { json, type Handler } from "./support/api";
import { wrap } from "./support/render";

// P6-TPL-4 Part A, C-1114: the Administration › What agents may do › Agent templates tab — the sources list (with
// Check now, Edit with token replace, Remove), the two-step Add dialog (Check, Back, errors in words) and the
// templates list (the chips, the apps and libraries it needs, Withdraw / Offer again).

vi.mock("next/navigation", () => ({
  useParams: () => ({}),
  useSearchParams: () => new URLSearchParams(),
}));

const T = "2026-01-01T00:00:00Z";
const SHA_B = "89abcdef0123456789abcdef0123456789abcdef";

const source = (over: Record<string, unknown> = {}) => ({
  id: "s1", name: "Acme templates", url: "https://github.com/acme/agent-templates.git", repo: "github.com/acme/agent-templates",
  ref: null, path: "", token_set: false, token_updated_at: null, homepage: null, contact: null,
  packages_url: "https://github.com/acme/agent-templates/releases/download/{version}/{id}-{version}.zip",
  update_interval: "1d", status: "ok", check_failed_at: null, check_failed_reason: null,
  last_checked_at: T, last_head_sha: SHA_B, created_at: T, updated_at: T,
  templates: [
    { template_id: "schedule-manager", dir: "schedule-manager", name: "Schedule manager", summary: "Keep the team's schedules tidy", head_state: "ok", head_version: "1.1.0", state: "ingested" },
  ],
  ...over,
});

const sourceTemplate = (over: Record<string, unknown> = {}) => ({
  template_id: "schedule-manager", dir: "schedule-manager", name: "Schedule manager", summary: "Keep the team's schedules tidy", head_state: "ok", head_version: "1.1.0", state: "ingested",
  ...over,
});

const preview = {
  url: "https://github.com/acme/agent-templates.git", repo: "github.com/acme/agent-templates", ref: "main",
  resolved: { kind: "branch", name: "main", default: true }, commit_sha: SHA_B, path: "templates",
  manifest: { name: "acme-templates", description: "Templates for the ops team", packages_url: "https://github.com/acme/agent-templates/releases/download/{version}/{id}-{version}.zip" },
  templates: [
    { template_id: "schedule-manager", dir: "schedule-manager", name: "Schedule manager", summary: "Keep the team's schedules tidy", version: "1.1.0", state: "new", problems: [] },
    { template_id: "broken", dir: "broken", name: "broken", summary: "", version: null, state: "new", problems: [{ code: "template_readme_missing", path: "broken/README.md", message: "The README.md file is missing." }] },
  ],
  truncated: false,
};

const adminTemplate = (over: Record<string, unknown> = {}) => ({
  template_id: "schedule-manager", name: "Schedule manager", summary: "Keep the team's schedules tidy",
  source: { id: "s1", name: "Acme templates" },
  current_version: "1.0.0", offered: true, removed_upstream: false, head_state: "ok", head_version: "1.1.0", head_problems: null,
  needs_apps: [], needs_libraries: [],
  versions: [],
  ...over,
});

const pending = {
  version: "1.1.0", status: "pending_approval", commit_sha: SHA_B, created_at: T, decided_at: null, decided_by: null, last_attempt_at: null, size_bytes: 1000, status_reason: null,
};

/** The row whose first cell starts with exactly `label`. */
const row = (table: HTMLElement, label: string) => within(table).getAllByRole("row").find((r) => r.querySelector("td")?.textContent?.startsWith(label) ?? false) as HTMLElement;

const type = (label: string, value: string) => fireEvent.change(screen.getByLabelText(label), { target: { value } });

/** Stubs fetch with the tab's GET routes plus `extra`; other requests go to `onWrite` (or 404). */
function stubFetch(
  items: { sources?: unknown[]; templates?: unknown[] },
  extra: Record<string, Handler> = {},
  onWrite?: (key: string, body: unknown) => void,
) {
  const calls: string[] = [];
  const spy = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const p = decodeURIComponent(url.pathname);
    const key = `${req.method} ${p}`;
    calls.push(key);
    const routes: Record<string, Handler> = {
      "GET /v1/admin/agent-template-sources": () => json(200, { items: items.sources ?? [] }),
      "GET /v1/admin/agent-templates": () => json(200, { items: items.templates ?? [] }),
      ...extra,
    };
    const h = routes[key];
    if (h) return h(req);
    if (onWrite && req.method !== "GET") {
      onWrite(key, JSON.parse((await req.text()) || "null"));
      return json(200, {});
    }
    return json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", spy);
  return calls;
}

const appSummary = (over: Record<string, unknown> = {}) => ({
  app_id: "github", name: "GitHub", family: null, description: "", available: true, connection_count: 0,
  credential_free: true, has_dark_icon: true, package_id: "github", package_version: "1.0.0", send_only: false, serves_library: true, app_restrictions: {},
  ...over,
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

// ---- the sources list -------------------------------------------------------------------------

describe("Agent templates: sources", () => {
  it("lists the sources in words: default branch, repository root, the head, the schedule, last checked, status and count", async () => {
    stubFetch({ sources: [source()] });
    wrap(<AdminTemplates />);
    const table = await screen.findByRole("table", { name: "Template repositories of this instance" });
    const heads = within(table).getAllByRole("columnheader").map((h) => h.textContent);
    expect(heads).toEqual(["Name", "Repository", "Branch or tag", "Folder", "Latest commit", "Check for updates", "Last checked", "Status", "Templates", "Actions"]);
    const r = row(table, "Acme templates");
    expect(within(r).getByText("github.com/acme/agent-templates")).toBeTruthy();
    expect(within(r).getByText("Default branch")).toBeTruthy();
    expect(within(r).getByText("Repository root")).toBeTruthy();
    expect(within(r).getByText("89abcde")).toBeTruthy();
    expect(within(r).getByText("Every day")).toBeTruthy();
    expect(within(r).getByText("Ok")).toBeTruthy();
    expect(within(r).getByText("1")).toBeTruthy();
    // the actions of a source the instance admin manages
    for (const name of ["Check now Acme templates", "Edit Acme templates", "Remove Acme templates"]) {
      expect(within(r).getByRole("button", { name })).toBeTruthy();
    }
  });

  it("words what a source that could not be read looks like, and a manual one", async () => {
    stubFetch({ sources: [source({ ref: "v2", path: "templates", update_interval: "never", last_checked_at: null, last_head_sha: null, status: "check_failed", check_failed_reason: "The repository needs an access token", templates: [sourceTemplate(), sourceTemplate({ template_id: "second", dir: "second" })] })] });
    wrap(<AdminTemplates />);
    const table = await screen.findByRole("table", { name: "Template repositories of this instance" });
    const r = row(table, "Acme templates");
    expect(within(r).getByText("v2")).toBeTruthy();
    expect(within(r).getByText("templates")).toBeTruthy();
    expect(within(r).getByText("Never (manual checks only)")).toBeTruthy();
    expect(within(r).getByText("Never")).toBeTruthy();
    expect(within(r).getByText("Can't be read")).toBeTruthy();
    expect(within(r).getByText("The repository needs an access token")).toBeTruthy();
    expect(within(r).getByText("2")).toBeTruthy();
  });

  it("checks now, removes with the sentence that agents keep working", async () => {
    const calls = stubFetch({ sources: [source()] }, {
      "POST /v1/admin/agent-template-sources/s1/check": () => json(200, { source_id: "s1", status: "up_to_date", commit_sha: SHA_B, ingested: [], waiting: [], problems: [], check_failed_reason: null }),
      "DELETE /v1/admin/agent-template-sources/s1": () => json(204, null),
    });
    wrap(<AdminTemplates />);
    fireEvent.click(await screen.findByRole("button", { name: "Check now Acme templates" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove Acme templates" }));
    expect(await screen.findByText(/Agents made from these templates keep working and do not change/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove repository" }));
    await waitFor(() =>
      expect(calls.filter((c) => !c.startsWith("GET"))).toEqual(["POST /v1/admin/agent-template-sources/s1/check", "DELETE /v1/admin/agent-template-sources/s1"]),
    );
  });

  it("edits the schedule, the branch and the folder", async () => {
    const bodies: unknown[] = [];
    stubFetch({ sources: [source()] }, {
      "PATCH /v1/admin/agent-template-sources/s1": async (r) => {
        bodies.push(await r.json());
        return json(200, source());
      },
    });
    wrap(<AdminTemplates />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Acme templates" }));
    const dlg = await screen.findByRole("dialog", { name: "Edit Acme templates" });
    expect(within(dlg).getByText("https://github.com/acme/agent-templates.git")).toBeTruthy();
    expect(within(dlg).getByText("No token.")).toBeTruthy();
    fireEvent.change(within(dlg).getByLabelText("Check for updates"), { target: { value: "1w" } });
    fireEvent.change(within(dlg).getByLabelText("Branch or tag"), { target: { value: "v2" } });
    fireEvent.change(within(dlg).getByLabelText("Folder"), { target: { value: "templates" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(bodies).toEqual([{ update_interval: "1w", ref: "v2", path: "templates" }]));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit Acme templates" })).toBeNull());
  });

  it("replaces the token with the fold's fields", async () => {
    const bodies: unknown[] = [];
    stubFetch({ sources: [source({ token_set: true, token_updated_at: T })] }, {
      "PATCH /v1/admin/agent-template-sources/s1": async (r) => {
        bodies.push(await r.json());
        return json(200, source({ token_set: true }));
      },
    });
    wrap(<AdminTemplates />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Acme templates" }));
    const dlg = await screen.findByRole("dialog", { name: "Edit Acme templates" });
    expect(within(dlg).getByText(/A token is saved/)).toBeTruthy();
    fireEvent.click(within(dlg).getByRole("button", { name: "Replace" }));
    fireEvent.change(within(dlg).getByLabelText("New access token"), { target: { value: "tok" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(bodies).toEqual([{ token: "tok" }]));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit Acme templates" })).toBeNull());
  });

  it("removes the token after a confirm, which sends `token: null`", async () => {
    const bodies: unknown[] = [];
    stubFetch({ sources: [source({ token_set: true, token_updated_at: T })] }, {
      "PATCH /v1/admin/agent-template-sources/s1": async (r) => {
        bodies.push(await r.json());
        return json(200, source({ token_set: false }));
      },
    });
    wrap(<AdminTemplates />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Acme templates" }));
    const dlg = await screen.findByRole("dialog", { name: "Edit Acme templates" });
    fireEvent.click(within(dlg).getByRole("button", { name: "Remove" }));
    expect(await screen.findByText(/Aurora can no longer read this repository if it is private/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove token" }));
    await waitFor(() => expect(bodies).toEqual([{ token: null }]));
  });

  it("an edit that changes nothing sends nothing", async () => {
    const calls = stubFetch({ sources: [source()] });
    wrap(<AdminTemplates />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Acme templates" }));
    const dlg = await screen.findByRole("dialog", { name: "Edit Acme templates" });
    fireEvent.click(within(dlg).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit Acme templates" })).toBeNull());
    expect(calls).not.toContain("PATCH /v1/admin/agent-template-sources/s1");
  });
});

// ---- the two-step Add -------------------------------------------------------------------------

describe("Agent templates: Add repository", () => {
  const open = async (extra: Record<string, Handler> = {}) => {
    stubFetch({}, extra);
    wrap(<AdminTemplates />);
    fireEvent.click(await screen.findByRole("button", { name: "Add a repository" }));
    return screen.findByRole("dialog", { name: "Add a template repository" });
  };

  it("step 1 asks where it is; Check sends the token and its user name only when they are filled", async () => {
    let body: Record<string, unknown> = {};
    const dlg = await open({
      "POST /v1/admin/agent-template-sources/preview": async (r) => {
        body = (await r.json()) as Record<string, unknown>;
        return json(200, preview);
      },
    });
    expect(within(dlg).getByText(/The address you clone, or the repository's page/)).toBeTruthy();
    const fold = within(dlg).getByText("This repository is private").closest("details") as HTMLDetailsElement;
    expect(fold.open).toBe(false);
    // the address is required
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    expect(await within(dlg).findByText("Enter the repository's address.")).toBeTruthy();
    type("Repository address", "https://github.com/acme/agent-templates");
    fireEvent.click(within(dlg).getByText("This repository is private"));
    fireEvent.change(screen.getByLabelText("Access token"), { target: { value: "tok" } });
    fireEvent.change(screen.getByLabelText("User name"), { target: { value: "git" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    await within(dlg).findByTestId("template-source-found");
    expect(body).toEqual({ url: "https://github.com/acme/agent-templates", token: "tok", token_username: "git" });
  });

  it("step 2 shows what Aurora found as plain text, with each template's state and problems; Add repository saves what was found", async () => {
    let created: Record<string, unknown> = {};
    const dlg = await open({
      "POST /v1/admin/agent-template-sources/preview": () => json(200, preview),
      "POST /v1/admin/agent-template-sources": async (r) => {
        created = (await r.json()) as Record<string, unknown>;
        return json(201, source());
      },
    });
    type("Repository address", "https://github.com/acme/agent-templates");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    const f = await within(dlg).findByTestId("template-source-found");
    expect(within(f).getByText("github.com/acme/agent-templates")).toBeTruthy();
    expect(within(f).getByText("Templates for the ops team")).toBeTruthy();
    expect(within(f).getByText("main (default branch)")).toBeTruthy();
    expect(within(f).getByText("89abcde")).toBeTruthy();
    expect(within(f).getByText("templates")).toBeTruthy();
    expect(within(f).getByText("Packages come from")).toBeTruthy();
    expect(within(f).getByText("https://github.com/acme/agent-templates/releases/download/{version}/{id}-{version}.zip")).toBeTruthy();
    expect(within(f).getByText("2 templates")).toBeTruthy();
    // each template with its version, state and problems in words; repository text is data, never markup
    expect(within(f).getByText("Version 1.1.0 · New")).toBeTruthy();
    expect(within(f).getByText("No version yet · New")).toBeTruthy();
    expect(within(f).getByText("broken/README.md: The README.md file is missing.")).toBeTruthy();
    expect(dlg.querySelector("img")).toBeNull();
    fireEvent.click(within(dlg).getByRole("button", { name: "Add repository" }));
    // what was found is what is added: the normalized address with the branch and folder the check resolved
    await waitFor(() => expect(created).toEqual({ url: "https://github.com/acme/agent-templates.git", ref: "main", path: "templates" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add a template repository" })).toBeNull());
  });

  it("Back keeps the fields; a second Check reuses them", async () => {
    const dlg = await open({ "POST /v1/admin/agent-template-sources/preview": () => json(200, preview) });
    type("Repository address", "https://github.com/acme/agent-templates");
    fireEvent.change(screen.getByLabelText("Branch or tag"), { target: { value: "v2" } });
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    await within(dlg).findByTestId("template-source-found");
    fireEvent.click(within(dlg).getByRole("button", { name: "Back" }));
    expect((screen.getByLabelText("Repository address") as HTMLInputElement).value).toBe("https://github.com/acme/agent-templates");
    expect((screen.getByLabelText("Branch or tag") as HTMLInputElement).value).toBe("v2");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    await within(dlg).findByTestId("template-source-found");
  });

  it("a refused address says it on the address field, in words per the reason", async () => {
    const dlg = await open({
      "POST /v1/admin/agent-template-sources/preview": () =>
        json(422, { error: { code: "template_url_invalid", message: "bad address", details: { reason: "not_https" } } }),
    });
    type("Repository address", "http://github.com/acme/agent-templates");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    expect(await within(dlg).findByText("Use the https address; it starts with https://.")).toBeTruthy();
  });

  it("an address refused only on Add repository is shown in step 2, where the address field is not", async () => {
    const dlg = await open({
      "POST /v1/admin/agent-template-sources/preview": () => json(200, preview),
      "POST /v1/admin/agent-template-sources": () =>
        json(422, { error: { code: "template_url_invalid", message: "bad address", details: { reason: "not_https" } } }),
    });
    type("Repository address", "https://github.com/acme/agent-templates");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    await within(dlg).findByTestId("template-source-found");
    fireEvent.click(within(dlg).getByRole("button", { name: "Add repository" }));
    const alert = await within(dlg).findByRole("alert");
    expect(alert.textContent).toContain("Use the https address; it starts with https://.");
    expect(alert.textContent).toContain("Go back to change the address.");
    // Back goes to the address field.
    fireEvent.click(within(dlg).getByRole("button", { name: "Back" }));
    expect(screen.getByLabelText("Repository address")).toBeTruthy();
  });

  it("an unreachable repository says why above the fields", async () => {
    const dlg = await open({
      "POST /v1/admin/agent-template-sources/preview": () =>
        json(422, { error: { code: "template_unreachable", message: "The repository could not be read: The branch or tag was not found.", details: { reason: "The branch or tag was not found" } } }),
    });
    type("Repository address", "https://github.com/acme/agent-templates");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    expect(await within(dlg).findByText("The repository could not be read: The branch or tag was not found.")).toBeTruthy();
  });

  it("a rate-limited check says when to try again", async () => {
    const dlg = await open({
      "POST /v1/admin/agent-template-sources/preview": () => json(429, { error: { code: "rate_limited", message: "slow down" } }),
    });
    type("Repository address", "https://github.com/acme/agent-templates");
    fireEvent.click(within(dlg).getByRole("button", { name: "Check" }));
    expect(await within(dlg).findByText("Aurora checks ten repositories a minute. Try again in a minute.")).toBeTruthy();
  });
});

// ---- the templates list -----------------------------------------------------------------------

describe("Agent templates: templates list", () => {
  function renderTemplates(items: unknown[], extra: Record<string, Handler> = {}) {
    const bodies: { key: string; body: unknown }[] = [];
    stubFetch(
      { templates: items },
      {
        "GET /v1/apps": () => json(200, { items: [appSummary(), appSummary({ app_id: "gitlab", name: "GitLab", package_id: "gitlab", serves_library: false })] }),
        ...extra,
      },
      (key, body) => bodies.push({ key, body }),
    );
    wrap(<AdminTemplates />);
    return bodies;
  }

  // (FIX-245) The waiting versions sit at the top of the tab as a "Needs you" strip, a Review button each.
  it("puts the versions waiting for review at the top as a Needs you strip, and nothing when none wait", async () => {
    renderTemplates([
      adminTemplate({ versions: [pending] }),
      adminTemplate({ template_id: "second", name: "Second", offered: false }),
    ]);
    const strip = await screen.findByTestId("templates-needs-you");
    expect(within(strip).getAllByTestId("needs-review-row")).toHaveLength(1);
    expect(within(strip).getAllByTestId("needs-review-row")[0]!.textContent).toContain("is waiting for review");
    const link = within(strip).getByRole("link", { name: "Review" });
    expect(link.getAttribute("href")).toBe("/admin/internet-access/templates/schedule-manager/1.1.0");

    cleanup();
    renderTemplates([adminTemplate()]);
    await screen.findByRole("table", { name: "Agent templates of this instance" });
    expect(screen.queryByTestId("templates-needs-you")).toBeNull();
  });

  it("shows the chips of C-1114: waiting for review with the count and the review link, waiting for its package, removed, head problems", async () => {
    renderTemplates([
      adminTemplate({
        versions: [
          pending,
          { ...pending, version: "1.2.0", status: "awaiting_package", created_at: "2026-02-01T00:00:00Z" },
        ],
        removed_upstream: true, head_state: "invalid", head_problems: [{ code: "template_yaml", dir: "broken", message: "template.yaml is not valid YAML." }],
      }),
    ]);
    const table = await screen.findByRole("table", { name: "Agent templates of this instance" });
    const r = row(table, "Schedule manager");
    const link = within(r).getByRole("link", { name: "1 version of Schedule manager waiting for review, review it" });
    expect(link.getAttribute("href")).toBe("/admin/internet-access/templates/schedule-manager/1.1.0");
    expect(within(r).getByText("Waiting for review (1)")).toBeTruthy();
    expect(within(r).getByText(/^Waiting for its package since/)).toBeTruthy();
    expect(within(r).getByText("Removed from the repository")).toBeTruthy();
    expect(within(r).getByText("The repository's newest version has problems")).toBeTruthy();
    expect(within(r).getByText("broken: template.yaml is not valid YAML.")).toBeTruthy();
    // the offered version is still the approved one
    expect(within(r).getByText("1.0.0")).toBeTruthy();
  });

  it("shows the apps and libraries it needs, with what this instance lacks in words", async () => {
    renderTemplates([
      adminTemplate({
        needs_apps: [{ app_id: "github", installed: false }, { app_id: "gitlab", installed: true }],
        needs_libraries: [{ url: "https://git.example.org/acme/skills.git", ref: "v1", path: "skills", added_here: false }],
      }),
    ]);
    const table = await screen.findByRole("table", { name: "Agent templates of this instance" });
    const r = row(table, "Schedule manager");
    const apps = within(r).getByRole("list", { name: "Apps Schedule manager needs" });
    expect(await within(apps).findByText("GitHub · Not installed here")).toBeTruthy();
    expect(within(apps).getByText("GitLab")).toBeTruthy();
    const libs = within(r).getByRole("list", { name: "Skill libraries Schedule manager needs" });
    expect(within(libs).getByText(/https:\/\/git\.example\.org\/acme\/skills\.git · v1 · skills · Not added here/)).toBeTruthy();
  });

  it("withdraws and offers again", async () => {
    const bodies = renderTemplates([adminTemplate(), adminTemplate({ template_id: "second", name: "Second", offered: false })]);
    fireEvent.click(await screen.findByRole("button", { name: "Withdraw Schedule manager" }));
    fireEvent.click(screen.getByRole("button", { name: "Offer Second again" }));
    await waitFor(() => expect(bodies.map((b) => b.body)).toEqual([{ offered: false }, { offered: true }]));
    expect(bodies.map((b) => b.key)).toEqual(["PATCH /v1/admin/agent-templates/schedule-manager", "PATCH /v1/admin/agent-templates/second"]);
  });

  it("a calm list when every template is in order: the offered version and no chips", async () => {
    renderTemplates([adminTemplate()]);
    const table = await screen.findByRole("table", { name: "Agent templates of this instance" });
    const r = row(table, "Schedule manager");
    expect(within(r).getByText("1.0.0")).toBeTruthy();
    expect(within(r).getByText("Acme templates")).toBeTruthy();
    expect(within(r).queryByRole("link")).toBeNull();
  });
});
