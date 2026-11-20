import { screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { appSummary, SUMMARY_FALLBACK } from "@/lib/app-summary";
import { mockFetch } from "./support/api";
import { wrap } from "./support/render";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/apps/packages/p1",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { PluginReview } from "@/components/admin/plugin-detail-view";
import type { PluginPackage } from "@/lib/api/plugins";

const base = {
  reaches: [] as string[],
  reaches_more: 0,
  permissions: { hooks: [] as string[], egress: {}, context: [] },
  summary: null as string | null,
  apps: [{ app_id: "github", name: "GitHub" }],
  package_id: "github",
};

/** A sample of manifests: each says something in plain words, none falls back or leaks a hook name. */
const SAMPLES: [string, Parameters<typeof appSummary>[0], string[]][] = [
  ["GitHub: reaches, signs in, purpose", { ...base, reaches: ["api.github.com"], permissions: { hooks: ["connect.exchange", "token.mint"], egress: {}, context: [] }, summary: "Can read and change repositories" }, ["Reaches api.github.com.", "Signs people in with GitHub.", "Can read and change repositories."]],
  ["token only", { ...base, permissions: { hooks: ["token.mint"], egress: {}, context: [] } }, ["Lets agents act in GitHub for the person who connected it."]],
  ["many hosts", { ...base, reaches: ["a.test", "b.test"], reaches_more: 2, summary: "Sends alerts." }, ["Reaches a.test, b.test and 2 more.", "Sends alerts."]],
];

describe("appSummary (P5-REFINE-18 #8, admin-7)", () => {
  it.each(SAMPLES)("%s", (_n, p, want) => {
    const got = appSummary(p);
    expect(got).toEqual(want);
    expect(got).not.toContain(SUMMARY_FALLBACK);
    expect(got.join(" ")).not.toMatch(/connect\.exchange|token\.mint/);
  });

  it("falls back to 'Review the details below.' when the manifest says nothing", () => {
    expect(appSummary(base)).toEqual([SUMMARY_FALLBACK]);
  });
});

describe("PluginReview opens on the summary (admin-7)", () => {
  const plugin = {
    id: "p1", package_id: "github", version: "1.4", publisher: "Aurora", digest: "sha256:abc", source: "directory", origin: "directory",
    signer: { kind: "trusted_key", name: "Aurora", key_id: "k1" }, status: "pending_approval", status_reason: null, violations: [], widenings: [],
    host_claims: [{ claim_key: "c", host: "{instance}", path_prefix: null, app_id: "github" }], impact: null, approved_by: null, approved_at: null,
    created_at: "2026-01-01T00:00:00Z", contract: 1, reaches: ["api.github.com"], reaches_more: 0, summary: "Can read and change repositories",
    apps: [{ app_id: "github", name: "GitHub" }], permissions: { hooks: ["connect.exchange", "token.mint"], egress: { "token.mint": ["api.github.com"] }, context: [] },
  } as unknown as PluginPackage;

  it("shows the name, 'App package, version', Approve and three sentences; hooks, claims and digest sit in a closed Technical details", () => {
    mockFetch({});
    wrap(<PluginReview plugin={plugin} />);
    const summary = screen.getByLabelText("Summary");
    expect(within(summary).getByText("App package, version 1.4")).toBeInTheDocument();
    expect(within(summary).getByRole("button", { name: "Approve" })).toBeEnabled();
    expect(within(summary).getByRole("heading", { name: "What this app does" })).toBeInTheDocument();
    expect(within(summary).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "Reaches api.github.com.",
      "Signs people in with GitHub.",
      "Can read and change repositories.",
    ]);
    const fold = screen.getByTestId("app-technical-details");
    expect(fold).not.toHaveAttribute("open");
    expect(within(fold).getByRole("button", { name: /Hooks/ })).toBeInTheDocument();
    expect(within(fold).getByText("sha256:abc")).toBeInTheDocument();
    expect(within(summary).queryByText(/connect\.exchange|sha256/)).toBeNull();
  });
});
