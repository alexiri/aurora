import { describe, expect, it } from "vitest";

import {
  availabilityWords,
  clientLabel,
  methodChip,
  methodSentence,
  needIsFixable,
  setupActions,
  setupHeadline,
} from "@/lib/apps/setup-copy";

type Method = Parameters<typeof methodChip>[0] & { label: string; kind: string; audience: "person" | "admin" };
const m = (over: Partial<Method> & { label: string; kind: string }): Method => ({ ready: true, needs: [], audience: "person", ...over });

const can = (over: Partial<Parameters<typeof setupActions>[1]["can"]> = {}) => ({
  register_workspace_client: false,
  register_org_client: false,
  register_instance: false,
  set_availability: false,
  connect_workspace: false,
  connect_org: false,
  ...over,
});

// GitHub in a workspace with no OAuth client anywhere: the token works, the sign-in and the GitHub App do not.
const GITHUB = [
  m({ label: "Personal access token", kind: "fields" }),
  m({ label: "Sign in with GitHub", kind: "oauth2", ready: false, needs: ["oauth_client"] }),
  m({ label: "GitHub App installation", kind: "hook", ready: false, needs: ["admin_material"], audience: "admin" }),
];

describe("setup copy: one sentence per way to connect (C-932)", () => {
  it("GitHub with no clients: token ready, sign-in needs a client, the App needs an admin", () => {
    const owner = can({ register_workspace_client: true });
    expect(GITHUB.map((x) => methodChip(x, "workspace", owner))).toEqual(["ready", "needs_setup", "admin_only"]);
    expect(methodSentence("GitHub", GITHUB[0]!, "workspace")).toBe("People can connect GitHub with a personal access token.");
    expect(methodSentence("GitHub", GITHUB[1]!, "workspace", { orgName: "Users" })).toBe(
      "Sign in with GitHub needs an OAuth client: none is set up for this workspace, Users or Aurora.",
    );
    expect(methodSentence("GitHub", GITHUB[2]!, "workspace")).toMatch(/^GitHub App installation needs details from whoever runs your GitHub organization/);
  });

  it("the organization view does not name the workspace", () => {
    expect(methodSentence("GitHub", GITHUB[1]!, "organization", { orgName: "Users" })).toBe(
      "Sign in with GitHub needs an OAuth client: none is set up for Users or Aurora.",
    );
  });

  it("Linear with a workspace client: sign-in is ready and reads as signing in", () => {
    const linear = [m({ label: "API key", kind: "fields" }), m({ label: "Sign in with Linear", kind: "oauth2" })];
    expect(linear.map((x) => methodChip(x, "workspace", can()))).toEqual(["ready", "ready"]);
    expect(methodSentence("Linear", linear[1]!, "workspace")).toBe("People can sign in with Linear.");
    expect(methodSentence("Linear", linear[0]!, "workspace")).toBe("People can connect Linear with an API key.");
    expect(setupHeadline("Linear", linear, "workspace")).toBe("People here can connect Linear with an API key or Sign in with Linear today.");
  });

  it("GitLab without a registered server needs a self-hosted app; only someone who may register one can fix it", () => {
    const gitlab = m({ label: "Personal access token", kind: "fields", ready: false, needs: ["instance"] });
    expect(methodSentence("GitLab", gitlab, "organization")).toBe("Personal access token needs a self-hosted GitLab server to be registered first.");
    expect(methodChip(gitlab, "organization", can({ register_instance: true }))).toBe("needs_setup");
    expect(methodChip(gitlab, "workspace", can({ register_workspace_client: true }))).toBe("admin_only");
  });

  it("a method switched off only because the app is not available says so", () => {
    const off = m({ label: "API key", kind: "fields", ready: false, needs: [] });
    expect(methodSentence("Linear", off, "workspace", { orgName: "Users", available: false })).toBe("API key is switched off: Linear is not available in Users.");
    expect(methodChip(off, "workspace", can({ register_workspace_client: true }))).toBe("admin_only");
  });

  it("the headline says what works today and what needs setup first", () => {
    expect(setupHeadline("GitHub", GITHUB, "workspace")).toBe(
      "People here can connect GitHub with a personal access token today. Sign in with GitHub needs setup first.",
    );
    expect(setupHeadline("GitHub", [GITHUB[1]!], "organization")).toBe("People cannot connect GitHub yet. Sign in with GitHub needs setup first.");
  });

  it("needs are fixable by level", () => {
    expect(needIsFixable("oauth_client", "workspace", can({ register_workspace_client: true }))).toBe(true);
    expect(needIsFixable("oauth_client", "organization", can({ register_workspace_client: true }))).toBe(false);
    expect(needIsFixable("admin_material", "organization", can({ register_org_client: true, register_instance: true }))).toBe(false);
  });
});

describe("setup copy: names and actions", () => {
  it("(C-946) a client is named by the ways of connecting it serves, never by its key", () => {
    expect(clientLabel("GitHub", [{ label: "Sign in with GitHub" }])).toBe("GitHub — for Sign in with GitHub");
    expect(clientLabel("GitHub", [{ label: "Sign in with GitHub" }, { label: "GitHub App" }])).toBe("GitHub — for Sign in with GitHub and GitHub App");
    expect(clientLabel("GitHub", [])).toBe("GitHub");
    expect(clientLabel("GitHub", undefined)).toBe("GitHub");
  });

  it("`can` decides which actions the page offers", () => {
    const base = { available: true };
    expect(setupActions("workspace", { ...base, can: can({ register_workspace_client: true, connect_workspace: true }) })).toEqual({
      connect: true, registerClient: true, registerInstance: false, setAvailability: false,
    });
    // a workspace owner who is not an organization admin: nothing above their level is offered
    expect(setupActions("workspace", { ...base, can: can({ register_workspace_client: true, connect_workspace: true, register_org_client: false }) }).setAvailability).toBe(false);
    expect(setupActions("organization", { ...base, can: can({ register_org_client: true, register_instance: true, set_availability: true, connect_org: true }) })).toEqual({
      connect: true, registerClient: true, registerInstance: true, setAvailability: true,
    });
    // the organization's view never offers a workspace's client or the organization's self-hosted app to a workspace view
    expect(setupActions("workspace", { ...base, can: can({ register_org_client: true, register_instance: true }) })).toMatchObject({ registerClient: false, registerInstance: false });
    // connecting an app the organization does not allow is not offered
    expect(setupActions("workspace", { available: false, can: can({ connect_workspace: true }) }).connect).toBe(false);
  });

  it("availability words", () => {
    expect(availabilityWords(true, "Users")).toBe("Available in Users");
    expect(availabilityWords(false)).toBe("Not available");
  });
});
