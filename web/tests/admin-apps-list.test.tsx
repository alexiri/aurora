import { fireEvent, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

let search = "";
vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/apps",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(search),
  useParams: () => ({}),
}));

import { PluginsView } from "@/components/admin/plugins-view";

/** (P5-POLISH-10, E5, C-1032) The apps list: Waiting, From Aurora, a way to every row's review. */

function pkg(id: string, name: string, over: Record<string, unknown> = {}) {
  return {
    id,
    package_id: `${id}-pkg`,
    version: "1.0.0",
    contract: 1,
    publisher: "aurora",
    digest: "d",
    signer: { kind: "builtin" },
    source: "builtin",
    origin: "aurora",
    summary: null,
    reaches: [],
    reaches_more: 0,
    apps: [{ app_id: id, name }],
    status: "active",
    status_reason: null,
    created_at: "2026-11-10T10:00:00Z",
    ...over,
  };
}

const AURORA = ["GitHub", "Google Workspace", "Sentry", "Slack", "Jira"].map((n, i) =>
  pkg(`a${i}`, n, { reaches: [`api.${n.split(" ")[0]!.toLowerCase()}.test`] }),
);

describe("the apps list", () => {
  it("lists uploaded packages under Waiting and the shipped apps under From Aurora, never package ids", async () => {
    search = "";
    mockFetch({
      "GET /v1/admin/plugins": () =>
        json(200, {
          items: [
            pkg("u1", "Acme tracker", {
              origin: "uploaded",
              source: "upload",
              status: "pending_approval",
              signer: { kind: "unsigned" },
              reaches: ["tracker.acme.test"],
            }),
            ...AURORA,
          ],
          next_cursor: null,
        }),
      "GET /v1/admin/addon-sources": () => json(200, { items: [] }),
    });
    wrap(<PluginsView />);
    expect(await screen.findByText("1 app is waiting for your approval.")).toBeTruthy();
    const waiting = screen.getByRole("region", { name: "1 app is waiting for your approval." });
    expect(within(waiting).getByText("Acme tracker")).toBeTruthy();
    expect(waiting.textContent).toContain("Uploaded · unsigned · reaches tracker.acme.test");
    // Review is a real link to the package's review page.
    expect(within(waiting).getByRole("link", { name: "Review Acme tracker" }).getAttribute("href")).toBe("/admin/apps/packages/u1");
    expect(within(waiting).getByRole("link", { name: "Acme tracker" }).getAttribute("href")).toBe("/admin/apps/packages/u1");

    const aurora = screen.getByRole("region", { name: "From Aurora" });
    expect(aurora.textContent).toContain("5 apps, approved with this release");
    // First three, "2 more" expands the rest.
    expect(within(aurora).getByText("GitHub")).toBeTruthy();
    expect(within(aurora).queryByText("Slack")).toBeNull();
    fireEvent.click(within(aurora).getByRole("button", { name: "2 more" }));
    expect(within(aurora).getByText("Slack")).toBeTruthy();
    // No package ids, versions or amber pending caps anywhere.
    expect(document.body.textContent).not.toContain("a0-pkg");
    expect(document.body.textContent).not.toMatch(/pending approval/i);
  });

  it("says no app is waiting when only Aurora's apps are there", async () => {
    search = "";
    mockFetch({
      "GET /v1/admin/plugins": () => json(200, { items: AURORA, next_cursor: null }),
      "GET /v1/admin/addon-sources": () => json(200, { items: [] }),
    });
    wrap(<PluginsView />);
    expect(await screen.findByText("No app is waiting.")).toBeTruthy();
    expect(screen.queryByRole("region", { name: /waiting for your approval/ })).toBeNull();
  });

  it("shows the toast after an approval", async () => {
    search = "approved=Acme%20tracker";
    mockFetch({
      "GET /v1/admin/plugins": () => json(200, { items: AURORA, next_cursor: null }),
      "GET /v1/admin/addon-sources": () => json(200, { items: [] }),
    });
    wrap(<PluginsView />);
    expect((await screen.findByTestId("approved-toast")).textContent).toContain("Acme tracker approved");
  });
});

// P5-REFINE-18 #12 (admin-11): A to Z, and one search with the count.
describe("the apps list: A to Z and search", () => {
  const serve = () =>
    mockFetch({
      "GET /v1/admin/plugins": () => json(200, { items: AURORA, next_cursor: null }),
      "GET /v1/admin/addon-sources": () => json(200, { items: [] }),
    });

  it("lists apps A to Z whatever order the server answers in", async () => {
    search = "";
    serve();
    wrap(<PluginsView />);
    const aurora = await screen.findByRole("region", { name: "From Aurora" });
    fireEvent.click(within(aurora).getByRole("button", { name: "2 more" }));
    const names = within(aurora)
      .getAllByRole("link", { name: /^(GitHub|Google Workspace|Jira|Sentry|Slack)$/ })
      .map((a) => a.textContent);
    expect(names).toEqual(["GitHub", "Google Workspace", "Jira", "Sentry", "Slack"]);
  });

  it("searches by name with the count in the placeholder, and says when nothing matches", async () => {
    search = "";
    serve();
    wrap(<PluginsView />);
    const box = await screen.findByRole("searchbox", { name: "Search apps" });
    expect(box).toHaveAttribute("placeholder", "Search 5 apps");
    fireEvent.change(box, { target: { value: "sen" } });
    const aurora = screen.getByRole("region", { name: "From Aurora" });
    expect(aurora.textContent).toContain("1 app, approved with this release");
    expect(within(aurora).getByText("Sentry")).toBeTruthy();
    fireEvent.change(box, { target: { value: "zzz" } });
    expect(screen.getByText('No app matches "zzz".')).toBeTruthy();
  });
});
