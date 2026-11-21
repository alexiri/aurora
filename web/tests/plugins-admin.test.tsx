import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ConflictGroup } from "@/components/admin/host-claims-view";
import { PluginDetailView, PluginReview } from "@/components/admin/plugin-detail-view";
import { UploadForm } from "@/components/admin/plugins-view";
import type { HostClaimConflict, PluginPackage } from "@/lib/api/plugins";
import { wrap } from "./support/render";

const push = vi.fn();
const replace = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, replace, back: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/admin/apps/p1",
  useSearchParams: () => new URLSearchParams(),
}));
import { json } from "./support/api";

interface Call {
  method: string;
  path: string;
  search: string;
  body: string | FormData | null;
}

function mockFetch(handler: (c: Call) => Response) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      let body: string | FormData | null = (init?.body as string | FormData | null | undefined) ?? null;
      if (req && req.method !== "GET" && req.method !== "DELETE") {
        const ct = req.headers.get("content-type") ?? "";
        body = ct.startsWith("multipart/") ? await req.clone().formData() : await req.clone().text();
      }
      const call: Call = {
        method: req?.method ?? init?.method ?? "GET",
        path: url.pathname,
        search: url.search,
        body,
      };
      calls.push(call);
      return handler(call);
    }),
  );
  return calls;
}

const plugin: PluginPackage = {
  id: "p1",
  package_id: "acme.git",
  version: "1.0.0",
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
  apps: [{ app_id: "git", name: "<b>Git</b>" }],
  permissions: { hooks: ["token.mint"], egress: { "token.mint": ["api.acme.test"] }, context: ["org.name"] },
  host_claims: [],
  widenings: [{ kind: "egress", detail: "adds api.acme.test" }],
  impact: null,
  approved_by: null,
  approved_at: null,
  created_at: "2026-01-01T00:00:00Z",
};

// (FIX-201, D-438, C-1120) An app's name links by package id; the page resolves it to a version's UUID.
describe("PluginDetailView with a package id", () => {
  const U1 = "00000000-0000-4000-8000-000000000001";
  const U2 = "00000000-0000-4000-8000-000000000002";
  const row = (id: string, status: string) => ({ ...plugin, id, package_id: "atlassian", status });

  it("replaces the address with the active version's UUID", async () => {
    replace.mockClear();
    const calls = mockFetch(() => json(200, { items: [row(U2, "pending_approval"), row(U1, "active")], next_cursor: null }));
    wrap(<PluginDetailView id="atlassian" />);
    await waitFor(() => expect(replace).toHaveBeenCalledWith(`/admin/apps/packages/${U1}`));
    expect(calls).toHaveLength(1);
    expect(calls[0]!.path).toBe("/v1/admin/plugins");
    expect(calls[0]!.search).toContain("package_id=atlassian");
    // usePlugin is never called with a package id
    expect(calls.some((c) => c.path === "/v1/admin/plugins/atlassian")).toBe(false);
  });

  it("falls back to the newest version when none is active", async () => {
    replace.mockClear();
    mockFetch(() => json(200, { items: [row(U2, "pending_approval"), row(U1, "superseded")], next_cursor: null }));
    wrap(<PluginDetailView id="atlassian" />);
    await waitFor(() => expect(replace).toHaveBeenCalledWith(`/admin/apps/packages/${U2}`));
  });

  it("says no such package is installed, not the form error", async () => {
    replace.mockClear();
    mockFetch(() => json(200, { items: [], next_cursor: null }));
    wrap(<PluginDetailView id="nope" />);
    expect(await screen.findByText("No app package called nope is installed.")).toBeInTheDocument();
    expect(screen.queryByText(/check the form/i)).toBeNull();
    expect(screen.getByRole("link", { name: /Administration › Apps/ })).toHaveAttribute("href", "/admin/apps?tab=apps");
    expect(replace).not.toHaveBeenCalled();
  });
});

describe("PluginReview", () => {
  it("enables approve at once, with every section open, and returns to the list with the app's name (E5)", async () => {
    push.mockClear();
    const calls = mockFetch(() => json(200, { ...plugin, status: "active", connections_reauth: 0 }));
    wrap(<PluginReview plugin={plugin} />);
    const approve = screen.getByRole("button", { name: "Approve" });
    expect(approve).toBeEnabled();
    for (const name of [/Hooks/, /Egress per hook/, /Context variables/, /Host claims/]) {
      expect(screen.getByRole("button", { name })).toHaveAttribute("aria-expanded", "true");
    }
    expect(screen.getByText(/adds api.acme.test/)).toBeInTheDocument();
    // plugin text is plain text, not markup
    expect(screen.getAllByText("<b>Git</b>", { exact: false }).length).toBeGreaterThan(0);
    fireEvent.click(approve);
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/admin/plugins/p1/approve")).toBe(true));
    await waitFor(() => expect(push).toHaveBeenCalledWith(expect.stringContaining("approved=")));
    expect(push.mock.calls[0]![0]).toContain("/admin/apps");
  });

  it("says reason codes and the {instance} placeholder in words", () => {
    mockFetch(() => json(200, plugin));
    wrap(
      <PluginReview
        plugin={{
          ...plugin,
          status_reason: "first_install",
          permissions: { hooks: [], egress: { h: ["{instance}"] }, context: [] },
          apps: [{ app_id: "git", name: "Git" }],
        }}
      />,
    );
    expect(screen.getByText("First install")).toBeInTheDocument();
    expect(screen.queryByText("(first_install)")).toBeNull();
    expect(screen.getByText(/your server's address/)).toBeInTheDocument();
    expect(screen.queryByText(/\{instance\}/)).toBeNull();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Git 1.0.0");
  });

  it("shows the impact banner and keeps approval enabled on a major upgrade", async () => {
    const major: PluginPackage = {
      ...plugin,
      version: "2.0.0",
      status_reason: "breaks_connections",
      impact: {
        major_upgrade: true,
        from_version: "1.0.0",
        connections_total: 5,
        connections_needing_reauth: 3,
        by_app: [{ app_id: "git", connections: 5, needing_reauth: 3 }],
      },
    };
    const calls = mockFetch(() => json(200, { ...major, status: "active", connections_reauth: 3 }));
    wrap(<PluginReview plugin={major} />);
    expect(screen.getByRole("status", { name: "Upgrade impact" })).toHaveTextContent("3 connections will need reconnecting");
    const approve = screen.getByRole("button", { name: "Approve" });
    expect(approve).toBeEnabled();
    expect(screen.getByText(/3 of 5 to reconnect/)).toBeInTheDocument();
    fireEvent.click(approve);
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/admin/plugins/p1/approve")).toBe(true));
    expect(await screen.findByText(/Approved\. 3 connections will need reconnecting/)).toBeInTheDocument();
  });

  it("shows the grant conversion banner and its section (P4-GRANTS)", async () => {
    const conv: PluginPackage = {
      ...plugin,
      version: "1.1.0",
      status_reason: "converts_grants",
      impact: {
        major_upgrade: false,
        from_version: "1.0.0",
        connections_total: 2,
        connections_needing_reauth: 0,
        grants_to_convert: 3,
        by_app: [{ app_id: "indico", connections: 2, needing_reauth: 0, grants_to_convert: 3, grant_workspaces: 2 }],
      },
    };
    const calls = mockFetch(() => json(200, { ...conv, status: "active", connections_reauth: 0 }));
    wrap(<PluginReview plugin={conv} />);
    expect(screen.getByRole("status", { name: "Grant conversion" })).toHaveTextContent(
      "3 agent grants will be converted from full to custom access",
    );
    // Not a major upgrade: no reconnect banner.
    expect(screen.queryByRole("status", { name: "Upgrade impact" })).toBeNull();
    const approve = screen.getByRole("button", { name: "Approve" });
    expect(approve).toBeEnabled();
    expect(screen.getByText(/indico/)).toBeInTheDocument();
    expect(screen.getByText(/3 grants in/)).toBeInTheDocument();
    fireEvent.click(approve);
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/admin/plugins/p1/approve")).toBe(true));
  });

  it("shows no impact banner or section for a non-major version", () => {
    mockFetch(() => json(200, plugin));
    wrap(
      <PluginReview
        plugin={{
          ...plugin,
          impact: { major_upgrade: false, from_version: "1.0.0", connections_total: 2, connections_needing_reauth: 0, by_app: [] },
        }}
      />,
    );
    expect(screen.queryByRole("status", { name: "Upgrade impact" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Impact on connections/ })).toBeNull();
  });

  it("asks for force confirmation on live_connections", async () => {
    const calls = mockFetch((c) =>
      c.method === "DELETE" && !c.search
        ? json(409, { error: { code: "live_connections", message: "x", details: { count: 3 } } })
        : c.method === "DELETE"
          ? json(204, null)
          : json(200, plugin),
    );
    wrap(<PluginReview plugin={plugin} />);
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    expect(await screen.findByText(/3 connection\(s\)/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Uninstall anyway" }));
    await waitFor(() =>
      expect(calls.some((c) => c.method === "DELETE" && c.search === "?force=true")).toBe(true),
    );
    expect(await screen.findByText(/App package uninstalled/)).toBeInTheDocument();
  });
});

describe("UploadForm", () => {
  it("renders violations from a 422", async () => {
    const calls = mockFetch(() =>
      json(422, {
        error: {
          code: "package_invalid",
          message: "That isn't an app package. Use a `.tar.gz` made with `aurora plugin pack`.",
          details: { violations: [{ code: "archive_corrupt", path: "/hooks/0", message: "invalid gzip header", sentence: "That isn't an app package. Use a `.tar.gz` made with `aurora plugin pack`." }] },
        },
      }),
    );
    const onCancel = vi.fn();
    wrap(<UploadForm onCancel={onCancel} />);
    const file = new File(["x"], "notes.txt");
    fireEvent.change(screen.getByLabelText("App package"), { target: { files: [file] } });
    expect(screen.getByTestId("chosen-package")).toHaveTextContent("notes.txt");
    expect(screen.getByRole("button", { name: "Choose another" })).toBeInTheDocument();
    // The native input no longer shows (and prints) the name a second time.
    expect(screen.getByLabelText("App package").className).toContain("sr-only");
    fireEvent.click(screen.getByRole("button", { name: "Upload" }));
    // (admin-12, C-1071) The sentence is shown; the code and the validator's words are under Technical details.
    // (P5-REFINE-20 #15) Said once: the one problem's sentence is the message, so no list repeats it.
    expect(await screen.findAllByText(/That isn't an app package/)).toHaveLength(1);
    // (batch 7) The backticked command is set as code, not printed with its backticks; Upload waits for another file; Cancel closes.
    const alert = screen.getByRole("alert");
    expect(alert.textContent).not.toContain("`");
    expect(Array.from(alert.querySelectorAll("code")).map((c) => c.textContent)).toEqual([".tar.gz", "aurora plugin pack"]);
    expect(alert.textContent).toContain("Use a .tar.gz made with aurora plugin pack.");
    expect(screen.getByRole("button", { name: "Upload" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalled();
    expect(screen.queryByRole("list", { name: "Violations" })).toBeNull();
    const fold = screen.getByText("Technical details").closest("details")!;
    expect(fold.textContent).toContain("archive_corrupt");
    expect(fold.textContent).toContain("invalid gzip header");
    expect((calls[0].body as FormData).get("package")).toBeTruthy();
    // Another file clears the error and enables Upload again.
    fireEvent.change(screen.getByLabelText("App package"), { target: { files: [new File(["y"], "b.tar.gz")] } });
    expect(screen.getByRole("button", { name: "Upload" })).toBeEnabled();
  });

  it("several problems: the message, then one sentence per problem, codes under Technical details", async () => {
    mockFetch(() =>
      json(422, {
        error: {
          code: "package_invalid",
          message: "That app package has 2 problems. The first: The package is missing manifest.yaml.",
          details: {
            violations: [
              { code: "file_missing", path: "manifest.yaml", message: "missing", sentence: "The package is missing manifest.yaml." },
              { code: "icon_not_png", path: "icon.png", message: "not a png", sentence: "The icon must be a square PNG." },
            ],
          },
        },
      }),
    );
    wrap(<UploadForm />);
    fireEvent.change(screen.getByLabelText("App package"), { target: { files: [new File(["x"], "a.tar.gz")] } });
    fireEvent.click(screen.getByRole("button", { name: "Upload" }));
    const list = await screen.findByRole("list", { name: "Violations" });
    expect(list.textContent).toContain("The icon must be a square PNG.");
    expect(list.textContent).not.toContain("icon_not_png");
  });
});

describe("ConflictGroup", () => {
  it("PUTs the group key and fingerprint", async () => {
    const c: HostClaimConflict = {
      group_key: "api.example.com/",
      fingerprint: "fp1",
      claims: [
        { package_id: "a.pkg", app_id: "a", claim_key: "k1" },
        { package_id: "b.pkg", app_id: "b", claim_key: "k2" },
      ],
      decision: null,
    };
    const calls = mockFetch(() => json(200, c));
    wrap(<ConflictGroup c={c} />);
    fireEvent.click(screen.getByRole("button", { name: /Pick b.pkg \/ b/ }));
    await waitFor(() => expect(calls.length).toBeGreaterThan(0));
    expect(calls[0].method).toBe("PUT");
    expect(calls[0].path).toBe("/v1/admin/host-claims/api.example.com%2F");
    expect(JSON.parse(calls[0].body as string)).toEqual({
      winner_package_id: "b.pkg",
      winner_app_id: "b",
      fingerprint: "fp1",
    });
  });
});
