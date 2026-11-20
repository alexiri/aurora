import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

import { accountRoutes, adminRoutes, orgRoutes, topicHref, url, wsRoutes } from "@/lib/routes";

describe("route helper (C-960)", () => {
  it("builds a query without empty values and keeps the anchor", () => {
    expect(url("/x", { a: "1", b: "", c: null, d: undefined, e: 0 }, "#h")).toBe("/x?a=1&e=0#h");
    expect(url("/x")).toBe("/x");
  });

  it("names every topic and tab at each level", () => {
    expect(adminRoutes.people("sign-in")).toBe("/admin/people?tab=sign-in");
    expect(adminRoutes.advanced("publishers")).toBe("/admin/apps?tab=advanced#publishers");
    expect(adminRoutes.package("p/1")).toBe("/admin/apps/packages/p%2F1");
    expect(adminRoutes.organization("o1")).toBe("/admin/people/organizations/o1");
    expect(orgRoutes("acme").people("members")).toBe("/o/acme/people?tab=members");
    expect(orgRoutes("acme").group("g1")).toBe("/o/acme/people/groups/g1");
    expect(orgRoutes("acme").accounts({ kind: "ai-key" })).toBe("/o/acme/apps?tab=accounts&kind=ai-key");
    expect(orgRoutes("acme").skillLibraries()).toBe("/o/acme/agents/skills/libraries");
    expect(wsRoutes("acme", "main").accounts(undefined, "requests")).toBe("/o/acme/w/main/apps?tab=accounts#requests");
    expect(wsRoutes("acme", "main").settings("api-keys")).toBe("/o/acme/w/main/settings?tab=api-keys");
    expect(wsRoutes("acme", "main").agent("daneel", { tab: "access", connect: "github" })).toBe(
      "/o/acme/w/main/agents/daneel?tab=access&connect=github",
    );
    expect(accountRoutes.profile).toBe("/account");
  });

  it("opens the Connect dialog on Apps and the Reconnect dialog on Accounts (C-842)", () => {
    expect(wsRoutes("a", "b").connect("github")).toBe("/o/a/w/b/apps?tab=apps&connect=github");
    expect(wsRoutes("a", "b").connect("github", "c1")).toBe("/o/a/w/b/apps?tab=accounts&reconnect=c1");
  });

  it("links a topic at each level; the organization's Apps link names its tab", () => {
    expect(topicHref("admin", "overview")).toBe("/admin");
    expect(topicHref("admin", "ai-models")).toBe("/admin/ai-models");
    expect(topicHref("org", "overview", "acme")).toBe("/o/acme");
    expect(topicHref("org", "apps", "acme")).toBe("/o/acme/apps?tab=accounts");
    expect(topicHref("workspace", "overview", "acme", "main")).toBe("/o/acme/w/main");
    expect(topicHref("workspace", "internet-access", "acme", "main")).toBe("/o/acme/w/main/internet-access");
  });

  it("maps a settings section to the topic tab that renders it (C-971)", () => {
    expect(adminRoutes.sectionPage("sign_in")).toBe("/admin/people?tab=sign-in");
    expect(adminRoutes.sectionPage("sign-in")).toBe("/admin/people?tab=sign-in");
    expect(adminRoutes.sectionPage("governance", "default_x")).toBe("/admin/internet-access?tab=ai-keys#default_x");
    expect(adminRoutes.sectionPage("agents")).toBe("/admin/internet-access?tab=defaults");
    expect(adminRoutes.sectionPage("terminal")).toBe("/admin/platform?tab=computers");
    expect(adminRoutes.governanceSetting("byo.enabled")).toBe("/admin/internet-access?tab=byo#byo.enabled");
    expect(adminRoutes.governanceSetting("governance.egress.minimum_posture")).toBe("/admin/internet-access?tab=internet-access#governance.egress.minimum_posture");
    expect(adminRoutes.sectionPage("nope")).toBe("/admin/settings");
  });
});

/** Old dashboard paths (before P5-IA-1) that no source file may spell: `routes.ts` and `legacy-routes.ts` own them. */
const OLD_PATHS: RegExp[] = [
  /["'`]\/admin\/(setup|users|orgs|identity|placement|plugins|addon-sources|trusted-keys|host-claims|instances|oauth-clients|connectors|secret-stores|runners|storage|audit)\b/,
  /["'`]\/admin\/settings\/[a-z\-$`{]/,
  /\/o\/\$\{[^}]+\}\/(credentials|policy|governance|members|groups|channels|api-keys|instances|skills|storage|audit)\b/,
  /\/o\/\$\{[^}]+\}\/w\/\$\{[^}]+\}\/(credentials|policy)\b/,
  /\$\{(base|orgBase|ctx\.base)\}\/(credentials|policy|governance|members|channels)\b/,
  /\/settings\?tab=access/,
];

function sources(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) sources(p, out);
    else if (/\.(ts|tsx)$/.test(name) && !/\.test\.|schema\.d\.ts$/.test(name)) out.push(p);
  }
  return out;
}

describe("no hard-coded old path (C-960)", () => {
  const src = join(__dirname, "..", "src");
  const owners = new Set(["lib/routes.ts", "lib/legacy-routes.ts"]);
  it("every internal href goes through the route helper", () => {
    const hits: string[] = [];
    for (const file of sources(src)) {
      const rel = relative(src, file);
      if (owners.has(rel)) continue;
      readFileSync(file, "utf8")
        .split("\n")
        .forEach((line, i) => {
          const t = line.trim();
          if (t.startsWith("//") || t.startsWith("*") || t.startsWith("/*")) return;
          if (OLD_PATHS.some((re) => re.test(line))) hits.push(`${rel}:${i + 1}: ${t}`);
        });
    }
    expect(hits).toEqual([]);
  });
});
