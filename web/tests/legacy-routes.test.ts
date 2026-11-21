import { describe, expect, it } from "vitest";

import { legacyTarget, STATIC_REDIRECTS } from "@/lib/legacy-routes";

/** Every old route of C-960 → its new place (query and anchor preserved). */
const CASES: [string, string][] = [
  // Administration
  ["/admin/setup", "/admin"],
  ["/admin/users", "/admin/people?tab=users"],
  ["/admin/orgs", "/admin/people?tab=organizations"],
  ["/admin/orgs/abc", "/admin/people/organizations/abc"],
  ["/admin/identity", "/admin/people?tab=sign-in"],
  ["/admin/placement", "/admin/people?tab=new-users"],
  ["/admin/plugins", "/admin/apps?tab=apps"],
  ["/admin/plugins/pkg1", "/admin/apps/packages/pkg1"],
  ["/admin/addon-sources", "/admin/apps?tab=advanced#package-sources"],
  ["/admin/trusted-keys", "/admin/apps?tab=advanced#publishers"],
  ["/admin/host-claims", "/admin/apps?tab=advanced#host-conflicts"],
  ["/admin/instances", "/admin/apps?tab=self-hosted"],
  ["/admin/oauth-clients", "/admin/apps?tab=oauth-clients"],
  ["/admin/connectors", "/admin/chat-platforms"],
  ["/admin/secret-stores", "/admin/platform?tab=secret-stores"],
  ["/admin/runners", "/admin/platform?tab=runners"],
  ["/admin/storage", "/admin/data?tab=storage"],
  ["/admin/audit", "/admin/data?tab=audit"],
  ["/admin/settings/governance", "/admin/internet-access?tab=ai-keys"],
  ["/admin/settings/llm", "/admin/ai-models"],
  // (P5-REFINE-18 #1) What agents may do is one page with four tabs; Computers moved under Health & runners.
  ["/admin/agents", "/admin/internet-access?tab=defaults"],
  ["/admin/agents?tab=defaults", "/admin/internet-access?tab=defaults"],
  ["/admin/agents?tab=skills", "/admin/internet-access?tab=defaults"],
  ["/admin/agents?tab=byo", "/admin/internet-access?tab=byo"],
  ["/admin/agents?tab=computers", "/admin/platform?tab=computers"],
  ["/admin/ai-models?tab=rules", "/admin/internet-access?tab=ai-keys"],
  ["/admin/internet-access?tab=limits", "/admin/internet-access?tab=internet-access"],
  ["/admin/settings/sign-in", "/admin/people?tab=sign-in"],
  ["/admin/settings/scim", "/admin/people?tab=sign-in"],
  ["/admin/settings/email", "/admin/platform?tab=email"],
  ["/admin/settings/secrets", "/admin/platform?tab=secret-stores"],
  ["/admin/settings/agents", "/admin/internet-access?tab=defaults"],
  ["/admin/settings/approvals", "/admin/internet-access?tab=defaults"],
  ["/admin/settings/terminal", "/admin/platform?tab=computers"],
  ["/admin/settings/previews", "/admin/platform?tab=computers"],
  ["/admin/settings/skills", "/admin/internet-access?tab=defaults"],
  ["/admin/settings/retention", "/admin/data?tab=retention"],
  ["/admin/settings/limits", "/admin/platform?tab=limits"],
  ["/admin/settings/unknown", "/admin/settings"],
  // Organization
  ["/o/acme/members", "/o/acme/people?tab=members"],
  ["/o/acme/groups", "/o/acme/people?tab=groups"],
  ["/o/acme/groups/g1", "/o/acme/people/groups/g1"],
  ["/o/acme/settings/directory", "/o/acme/people?tab=directory-sync"],
  ["/o/acme/credentials", "/o/acme/apps?tab=accounts"],
  ["/o/acme/credentials?tab=secrets", "/o/acme/apps?tab=accounts"],
  ["/o/acme/credentials?tab=connections", "/o/acme/apps?tab=accounts"],
  ["/o/acme/credentials?tab=llm", "/o/acme/apps?tab=accounts&kind=ai-key"],
  ["/o/acme/credentials?tab=oauth", "/o/acme/apps?tab=oauth-clients"],
  ["/o/acme/apps", "/o/acme/apps?tab=apps"],
  ["/o/acme/governance", "/o/acme/ai-models"],
  ["/o/acme/governance?tab=model", "/o/acme/ai-models"],
  ["/o/acme/governance?tab=compliance", "/o/acme/ai-models"],
  ["/o/acme/governance?tab=egress", "/o/acme/internet-access"],
  ["/o/acme/governance?tab=apps", "/o/acme/apps?tab=apps"],
  ["/o/acme/governance?tab=byo", "/o/acme/agents?tab=byo"],
  ["/o/acme/policy", "/o/acme/internet-access"],
  ["/o/acme/skills", "/o/acme/agents?tab=skills"],
  ["/o/acme/skills/libraries", "/o/acme/agents/skills/libraries"],
  ["/o/acme/settings/previews", "/o/acme/agents?tab=byo"],
  ["/o/acme/channels", "/o/acme/chat-platforms"],
  ["/o/acme/instances", "/o/acme/apps?tab=self-hosted"],
  ["/o/acme/api-keys", "/o/acme/settings?tab=api-keys"],
  ["/o/acme/audit", "/o/acme/data?tab=audit"],
  ["/o/acme/storage", "/o/acme/data?tab=storage"],
  // Workspace
  ["/o/acme/w/main/credentials", "/o/acme/w/main/apps?tab=accounts"],
  ["/o/acme/w/main/credentials?tab=secrets", "/o/acme/w/main/apps?tab=accounts"],
  ["/o/acme/w/main/credentials?tab=connections", "/o/acme/w/main/apps?tab=accounts"],
  ["/o/acme/w/main/credentials?tab=llm", "/o/acme/w/main/apps?tab=accounts&kind=ai-key"],
  ["/o/acme/w/main/credentials?tab=apps", "/o/acme/w/main/apps?tab=apps"],
  ["/o/acme/w/main/credentials?tab=oauth", "/o/acme/w/main/apps?tab=apps"],
  ["/o/acme/w/main/credentials?tab=proposals", "/o/acme/w/main/apps?tab=accounts#requests"],
  ["/o/acme/w/main/policy", "/o/acme/w/main/internet-access"],
  ["/o/acme/w/main/settings?tab=access", "/o/acme/w/main/people"],
  ["/o/acme/w/main/apps/github/connect", "/o/acme/w/main/apps?tab=apps&connect=github"],
  ["/o/acme/w/main/apps/github/connect?reconnect=c1", "/o/acme/w/main/apps?tab=accounts&reconnect=c1"],
];

describe("legacyTarget (C-960)", () => {
  it.each(CASES)("%s → %s", (from, to) => {
    const [path, search = ""] = from.split("?");
    expect(legacyTarget(path!, search)).toBe(to);
  });

  it("keeps the query parameters of the new pages and the anchor", () => {
    expect(legacyTarget("/o/acme/w/main/credentials", "tab=apps&connect=github")).toBe("/o/acme/w/main/apps?tab=apps&connect=github");
    expect(legacyTarget("/o/acme/credentials", "tab=connections&reconnect=c9")).toBe("/o/acme/apps?tab=accounts&reconnect=c9");
    expect(legacyTarget("/admin/plugins", "package=x", "top")).toBe("/admin/apps?tab=apps&package=x#top");
    expect(legacyTarget("/admin/settings/governance", "", "#default_action")).toBe("/admin/internet-access?tab=ai-keys#default_action");
  });

  it("answers null for a page that is not an old route", () => {
    for (const p of [
      "/admin",
      "/admin/people",
      "/admin/ai-models",
      "/admin/ai-models?tab=endpoints",
      "/admin/internet-access",
      "/admin/internet-access?tab=ai-keys",
      "/admin/platform?tab=computers",
      "/admin/settings",
      "/o/acme",
      "/o/acme/apps?tab=accounts",
      "/o/acme/settings",
      "/o/acme/w/main/settings",
      "/o/acme/w/main/settings?tab=api-keys",
      "/o/acme/w/main/agents/daneel",
      "/account",
    ]) {
      const [path, search = ""] = p.split("?");
      expect(legacyTarget(path!, search), p).toBeNull();
    }
  });

  it("agrees with every static redirect next.config answers", () => {
    for (const r of STATIC_REDIRECTS) {
      const sample = r.source.replace(/:org/g, "acme").replace(/:ws/g, "main").replace(/:id/g, "x1");
      const want = r.destination.replace(/:org/g, "acme").replace(/:ws/g, "main").replace(/:id/g, "x1");
      expect(legacyTarget(sample), r.source).toBe(want);
    }
  });
});
