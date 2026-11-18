import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AgentGuideSection } from "@/components/admin/agent-guide-section";
import { PluginReview } from "@/components/admin/plugin-detail-view";
import type { PluginAgentGuide, PluginPackage } from "@/lib/api/plugins";
import { wrap } from "./support/render";

/** (P5-GUIDE-3, C-648) The agent guide section: structure, change marks, inert text, fetch on selection. */

interface Call {
  method: string;
  path: string;
}

function mockFetch(handler: (c: Call) => Response) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const call: Call = { method: req?.method ?? init?.method ?? "GET", path: url.pathname };
      calls.push(call);
      return handler(call);
    }),
  );
  return calls;
}

const text = (status: number, body: string) =>
  new Response(body, { status, headers: { "content-type": "text/plain; charset=utf-8" } });

const full: PluginAgentGuide = {
  app_id: "git",
  summary: "Projects and pipelines.",
  instructions: "Use numeric ids.",
  files: [
    { file: "api.md", title: "REST conventions", kind: "reference", size: 2048, sha256: "a" },
    { file: "retry.md", title: "Retry the failed jobs", kind: "recipe", size: 512, sha256: "b" },
    { file: "data.json", title: "Sample data", kind: "reference", size: 10, sha256: "c" },
  ],
  operations: { file: "operations.json", size: 4096, sha256: "d", count: 37 },
  changed: {
    summary: true,
    instructions: false,
    operations: false,
    files: [
      { file: "retry.md", change: "added" },
      { file: "api.md", change: "changed" },
      { file: "old.md", change: "removed" },
    ],
  },
};

const plugin: PluginPackage = {
  id: "p1",
  package_id: "acme.git",
  version: "1.1.0",
  contract: 1,
  publisher: "Acme",
  digest: "abc",
  signer: { kind: "trusted_key", key_id: "k1", name: "Acme key" },
  source: "upload",
  status: "pending_approval",
  status_reason: null,
  violations: [],
  apps: [{ app_id: "git", name: "Git" }],
  permissions: { hooks: [], egress: {}, context: [] },
  host_claims: [],
  widenings: [],
  impact: null,
  approved_by: null,
  approved_at: null,
  created_at: "2026-01-01T00:00:00Z",
  agent_guides: [full],
};

describe("AgentGuideSection", () => {
  it("shows the summary, essentials, recipes before reference files, the operations file and every change mark", () => {
    mockFetch(() => text(200, ""));
    wrap(<AgentGuideSection plugin={plugin} />);
    expect(screen.getByText("Agents that use this app can read this text. It is written by the publisher.")).toBeInTheDocument();
    const app = screen.getByTestId("agent-guide-git");
    expect(within(app).getByText("Projects and pipelines.")).toBeInTheDocument();
    expect(within(app).getByRole("region", { name: "Essentials of Git" })).toHaveTextContent("Use numeric ids.");
    const headings = within(app)
      .getAllByRole("heading", { level: 5 })
      .map((h) => h.textContent);
    expect(headings.indexOf("Recipes")).toBeLessThan(headings.indexOf("Reference files"));
    expect(within(app).getByText("37 operations, 4.0 KiB")).toBeInTheDocument();
    // Change marks: summary, the added and changed files, the removed one.
    expect(within(app).getAllByText("Changed").length).toBe(2);
    expect(within(app).getAllByText("Added")).toHaveLength(1);
    expect(within(app).getAllByText("Removed").length).toBeGreaterThanOrEqual(1);
    expect(within(app).getByText("old.md")).toBeInTheDocument();
  });

  it("says so for an app with no guide and for a missing summary", () => {
    mockFetch(() => text(200, ""));
    const empty: PluginAgentGuide = { app_id: "bare", summary: null, instructions: "", files: [], operations: null, changed: null };
    const noSummary: PluginAgentGuide = { ...empty, app_id: "ess", instructions: "Essentials only." };
    wrap(
      <AgentGuideSection
        plugin={{ ...plugin, apps: [{ app_id: "bare", name: "Bare" }, { app_id: "ess", name: "Ess" }], agent_guides: [empty, noSummary] }}
      />,
    );
    expect(within(screen.getByTestId("agent-guide-bare")).getByText("This app ships no agent guide.")).toBeInTheDocument();
    expect(within(screen.getByTestId("agent-guide-ess")).getByText("No summary: the app description is used.")).toBeInTheDocument();
  });

  it("fetches a file only when it is selected and shows hostile text as inert text", async () => {
    const hostile = [
      "<script>window.__pwned = 1</script>",
      '<img src=x onerror="window.__pwned = 2">',
      "[click me](javascript:window.__pwned=3)",
      "https://evil.example/path",
      "# heading",
    ].join("\n");
    const calls = mockFetch(() => text(200, hostile));
    const { container } = wrap(<AgentGuideSection plugin={plugin} />);
    expect(calls).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "api.md" }));
    const viewer = await screen.findByRole("region", { name: "File api.md" });
    expect(calls).toEqual([{ method: "GET", path: "/v1/admin/plugins/p1/agent-guides/git/api.md" }]);
    expect(viewer).toHaveTextContent("<script>window.__pwned = 1</script>");
    expect(viewer).toHaveTextContent("[click me](javascript:window.__pwned=3)");
    // Nothing became an element: no script, no image, no link, no heading from the text.
    expect(container.querySelector("script, img, iframe")).toBeNull();
    expect(viewer.querySelector("*")).toBeNull();
    expect(container.querySelector("a[href]")).toBeNull();
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
    // Another selection is a second request.
    fireEvent.click(screen.getByRole("button", { name: "retry.md" }));
    await screen.findByRole("region", { name: "File retry.md" });
    expect(calls).toHaveLength(2);
  });

  it("shows an error when the file cannot be read and an empty-file note", async () => {
    mockFetch((c) => (c.path.endsWith("/api.md") ? text(404, "") : text(200, "")));
    wrap(<AgentGuideSection plugin={plugin} />);
    fireEvent.click(screen.getByRole("button", { name: "api.md" }));
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "data.json" }));
    expect(await screen.findByText("This file is empty.")).toBeInTheDocument();
  });
});

describe("PluginReview and the agent guide", () => {
  it("makes the agent guide part of the mandatory reading, with no second confirmation", async () => {
    const calls = mockFetch(() => new Response(JSON.stringify({ ...plugin, status: "active" }), { status: 200, headers: { "content-type": "application/json" } }));
    wrap(<PluginReview plugin={plugin} />);
    const approve = screen.getByRole("button", { name: "Approve" });
    for (const name of [/Hooks/, /Egress per hook/, /Context variables/, /Host claims/]) {
      fireEvent.click(screen.getByRole("button", { name }));
    }
    expect(approve).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /What agents can read/ }));
    expect(approve).toBeEnabled();
    fireEvent.click(approve);
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/admin/plugins/p1/approve")).toBe(true));
  });

  it("has no guide section for a package without guide text", () => {
    mockFetch(() => text(200, ""));
    wrap(<PluginReview plugin={{ ...plugin, agent_guides: [] }} />);
    expect(screen.queryByRole("button", { name: /What agents can read/ })).toBeNull();
  });
});
