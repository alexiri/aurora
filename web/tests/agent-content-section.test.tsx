import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AgentContentSection, skillsChangeSummary } from "@/components/admin/agent-content-section";
import { PluginReview } from "@/components/admin/plugin-detail-view";
import type { PluginAgent, PluginPackage } from "@/lib/api/plugins";
import { wrap } from "./support/render";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), back: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/admin/apps/p1",
  useSearchParams: () => new URLSearchParams(),
}));

/** (C-1136) The agent section of a package: structure, change marks, inert text, fetch on selection. */

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

const full: PluginAgent = {
  app_id: "git",
  summary: "Projects and pipelines.",
  instructions: "Use numeric ids.",
  skills: [
    {
      name: "git-api",
      description: "How the REST API is addressed",
      size: 2058,
      files: [
        { path: "SKILL.md", size: 2048, sha256: "a", executable: false },
        { path: "scripts/run.sh", size: 10, sha256: "c", executable: true },
      ],
    },
    { name: "git-retry", description: "Retry the failed jobs", size: 512, files: [{ path: "SKILL.md", size: 512, sha256: "b", executable: false }] },
  ],
  operations: { file: "operations.json", size: 4096, sha256: "d", count: 37 },
  changed: {
    summary: true,
    instructions: false,
    operations: false,
    skills: [
      { name: "git-retry", change: "added", files: [{ path: "SKILL.md", change: "added" }] },
      {
        name: "git-api",
        change: "changed",
        files: [{ path: "SKILL.md", change: "changed" }, { path: "scripts/run.sh", change: "changed" }, { path: "scripts/run.sh", change: "mode" }],
      },
      { name: "git-old", change: "removed", files: [{ path: "SKILL.md", change: "removed" }] },
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
  origin: "uploaded",
  reaches: [],
  reaches_more: 0,
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
  agent: [full],
};

describe("AgentContentSection", () => {
  it("shows the summary, essentials, operations, the skills with scripts marked and every change mark", () => {
    mockFetch(() => text(200, ""));
    wrap(<AgentContentSection plugin={plugin} />);
    expect(screen.getByText(/Agents that use this app get these skills\. They are written by the publisher/)).toBeInTheDocument();
    const app = screen.getByTestId("agent-content-git");
    expect(within(app).getByText("Projects and pipelines.")).toBeInTheDocument();
    expect(within(app).getByRole("region", { name: "Essentials of Git" })).toHaveTextContent("Use numeric ids.");
    expect(within(app).getByText("37 operations, 4.0 KiB")).toBeInTheDocument();
    expect(within(app).getByRole("button", { name: "operations.json" })).toBeInTheDocument();
    expect(within(app).getByRole("heading", { name: "Skills" })).toBeInTheDocument();
    // The skill rows: name, size, description; scripts marked on the skill and on the file.
    const api = within(app).getByTestId("agent-skill-git-api");
    expect(api).toHaveTextContent("REST API is addressed");
    expect(within(api).getAllByText("Script")).toHaveLength(2);
    expect(within(api).getByText("10 B")).toBeInTheDocument();
    // Change marks: the summary and the changed skill, the added skill, the mode pair, the removed skill.
    expect(within(app).getByTestId("agent-skill-git-retry")).toHaveTextContent("Added");
    expect(within(api).getByText("Now a script")).toBeInTheDocument();
    expect(within(api).getAllByText("Changed")).toHaveLength(3); // the skill, SKILL.md and run.sh
    expect(within(app).getAllByText("Changed")).toHaveLength(4); // + the summary
    expect(within(app).getByText("git-old")).toBeInTheDocument();
    expect(within(app).getByText("Removed")).toBeInTheDocument();
  });

  it("marks a skill whose name another app owns as not given to agents, and only that one (FIX-221)", () => {
    mockFetch(() => text(200, ""));
    const claimed: PluginAgent = {
      ...full,
      changed: null,
      skills: [
        { ...full.skills[0], claimed_by: { app_id: "git-x", app_name: "Git X" } },
        full.skills[1],
      ],
    };
    wrap(<AgentContentSection plugin={{ ...plugin, agent: [claimed] }} />);
    const app = screen.getByTestId("agent-content-git");
    expect(within(app).getByTestId("agent-skill-claimed-git-api")).toHaveTextContent(
      "Not given to agents: the name belongs to the Git X app",
    );
    expect(within(app).queryByTestId("agent-skill-claimed-git-retry")).toBeNull();
  });

  it("says 'no longer a script' for a file that lost the bit, and lists a removed file of a changed skill", () => {
    mockFetch(() => text(200, ""));
    const agent: PluginAgent = {
      ...full,
      changed: {
        summary: false,
        instructions: false,
        operations: true,
        skills: [{ name: "git-api", change: "changed", files: [{ path: "scripts/run.sh", change: "mode" }, { path: "scripts/old.sh", change: "removed" }] }],
      },
      skills: [{ ...full.skills[0]!, files: [{ path: "SKILL.md", size: 5, sha256: "a", executable: false }, { path: "scripts/run.sh", size: 10, sha256: "c", executable: false }] }],
    };
    wrap(<AgentContentSection plugin={{ ...plugin, agent: [agent] }} />);
    expect(screen.getByText("No longer a script")).toBeInTheDocument();
    expect(screen.queryByText("Now a script")).toBeNull();
    expect(screen.getByText("git-api/scripts/old.sh")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /Operations/ })).toHaveTextContent("Changed");
  });

  it("counts the skills of a pending version for the line above Approve", () => {
    expect(skillsChangeSummary(plugin)).toEqual({ added: 1, changed: 1, removed: 1, scripts: 1 });
    expect(skillsChangeSummary({ ...plugin, agent: [{ ...full, changed: null }] })).toBeNull();
    expect(skillsChangeSummary({ ...plugin, agent: [{ ...full, changed: { summary: true, instructions: false, operations: false, skills: [] } }] })).toBeNull();
  });

  it("says so for an app with nothing for agents and for a missing summary", () => {
    mockFetch(() => text(200, ""));
    const empty: PluginAgent = { app_id: "bare", summary: null, instructions: "", skills: [], operations: null, changed: null };
    const noSummary: PluginAgent = { ...empty, app_id: "ess", instructions: "Essentials only." };
    wrap(
      <AgentContentSection
        plugin={{ ...plugin, apps: [{ app_id: "bare", name: "Bare" }, { app_id: "ess", name: "Ess" }], agent: [empty, noSummary] }}
      />,
    );
    expect(within(screen.getByTestId("agent-content-bare")).getByText("This app gives agents nothing but its description.")).toBeInTheDocument();
    expect(within(screen.getByTestId("agent-content-ess")).getByText("No summary: the app description is used.")).toBeInTheDocument();
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
    const { container } = wrap(<AgentContentSection plugin={plugin} />);
    expect(calls).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "git-api/SKILL.md" }));
    const viewer = await screen.findByRole("region", { name: "File git-api/SKILL.md" });
    expect(calls).toEqual([{ method: "GET", path: "/v1/admin/plugins/p1/apps/git/skills/git-api/file" }]);
    expect(viewer).toHaveTextContent("<script>window.__pwned = 1</script>");
    expect(viewer).toHaveTextContent("[click me](javascript:window.__pwned=3)");
    // Nothing became an element: no script, no image, no link, no heading from the text.
    expect(container.querySelector("script, img, iframe")).toBeNull();
    expect(viewer.querySelector("*")).toBeNull();
    expect(container.querySelector("a[href]")).toBeNull();
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
    // Another selection is a second request.
    fireEvent.click(screen.getByRole("button", { name: "git-retry/SKILL.md" }));
    await screen.findByRole("region", { name: "File git-retry/SKILL.md" });
    expect(calls).toHaveLength(2);
  });

  it("shows an error when the file cannot be read and an empty-file note", async () => {
    mockFetch((c) => (c.path.includes("/skills/git-api/") ? text(404, "") : text(200, "")));
    wrap(<AgentContentSection plugin={plugin} />);
    fireEvent.click(screen.getByRole("button", { name: "git-api/SKILL.md" }));
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "operations.json" }));
    expect(await screen.findByText("This file is empty.")).toBeInTheDocument();
  });
});

describe("PluginReview and the agent section", () => {
  it("shows For agents open, the skills line above Approve, and approval needs no second confirmation", async () => {
    const calls = mockFetch(() => new Response(JSON.stringify({ ...plugin, status: "active" }), { status: 200, headers: { "content-type": "application/json" } }));
    wrap(<PluginReview plugin={plugin} />);
    const approve = screen.getByRole("button", { name: "Approve" });
    expect(screen.getByRole("heading", { name: "For agents", level: 2 })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /What agents get/ })).toHaveAttribute("aria-expanded", "true");
    const line = screen.getByTestId("skills-changed-line");
    expect(line).toHaveTextContent("Skills for agents changed: 1 added, 1 changed, 1 removed, 1 script");
    expect(within(line).getByRole("link", { name: "Skills for agents changed" })).toHaveAttribute("href", "#for-agents");
    // The line sits before the buttons in the page.
    expect(line.compareDocumentPosition(approve) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(approve).toBeEnabled();
    fireEvent.click(approve);
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/admin/plugins/p1/approve")).toBe(true));
  });

  it("has no skills line when no skill changed, and none once the version is active", () => {
    mockFetch(() => text(200, ""));
    const quiet = { ...full, changed: { summary: true, instructions: false, operations: false, skills: [] } };
    const { unmount } = wrap(<PluginReview plugin={{ ...plugin, agent: [quiet] }} />);
    expect(screen.queryByTestId("skills-changed-line")).toBeNull();
    unmount();
    wrap(<PluginReview plugin={{ ...plugin, status: "active" }} />);
    expect(screen.queryByTestId("skills-changed-line")).toBeNull();
  });

  it("has no For agents section for a package that gives agents nothing", () => {
    mockFetch(() => text(200, ""));
    wrap(<PluginReview plugin={{ ...plugin, agent: [] }} />);
    expect(screen.queryByRole("heading", { name: "For agents" })).toBeNull();
    expect(screen.queryByRole("button", { name: /What agents get/ })).toBeNull();
  });
});
