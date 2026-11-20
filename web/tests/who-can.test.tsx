import { cleanup, render, screen } from "@testing-library/react";
import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { FieldControl } from "@/components/apps/dynamic-form";
import { optionalNextLinks } from "@/components/onboarding/getting-started";
import type { FormField } from "@/lib/api/apps";
import { EmptyState, ErrorState } from "@/components/states";
import { fieldLabel } from "@/components/ui/field";
import { MainAction } from "@/components/ui/main-action";
import { Tabs } from "@/components/ui/tabs";
import { WhoCan } from "@/components/ui/who-can";
import { ApiError } from "@/lib/api/client";
import { ERROR_MESSAGES, NETWORK_SENTENCE, errorMessage, isTransportFailure } from "@/lib/api/errors";
import {
  ROLE_PHRASE,
  askSentence,
  forbiddenError,
  onlySentence,
  orgRoleFor,
  whoCanFromContacts,
  whoCanOf,
  type WhoCanRole,
  type WhoCanValue,
} from "@/lib/who-can";

afterEach(cleanup);

const who = (role: WhoCanRole, names: string[], over: Partial<WhoCanValue> = {}): WhoCanValue => ({
  action: "add_account",
  role,
  people: names.map((name) => ({ name })),
  you_can: false,
  ...over,
});

describe("WhoCan (C-981)", () => {
  it("has the fixed role phrases", () => {
    expect(ROLE_PHRASE).toEqual({
      workspace_owner: "workspace owners",
      workspace_approver: "workspace owners and approvers",
      org_admin: "organization owners and admins",
      org_owner: "organization owners",
      instance_admin: "an Aurora administrator",
    });
  });

  // (D-407) An admin is a role, never a login: several people can hold it, so no name is printed anywhere.
  it.each([
    ["workspace_owner", "Ask a workspace owner."],
    ["workspace_approver", "Ask a workspace owner or approver."],
    ["org_admin", "Ask an organization admin."],
    ["org_owner", "Ask an organization owner."],
  ] as const)("asks the role of %s and never prints a name", (role, sentence) => {
    render(<WhoCan value={who(role, ["Ada Lovelace", "Alan Turing"])} />);
    expect(screen.getByText(sentence)).toBeInTheDocument();
    expect(screen.queryByText(/Ada Lovelace|Alan Turing/)).toBeNull();
  });

  it("askSentence prints the role whatever people it is given, never a name", () => {
    expect(askSentence(who("org_admin", ["Ada"]))).toBe("Ask an organization admin.");
    expect(askSentence(who("org_admin", ["Ada", "Alan", "Grace"]))).toBe("Ask an organization admin.");
    expect(askSentence(who("org_admin", ["Ada", "Alan", "Grace"]))).not.toMatch(/Ada|Alan|Grace/);
    expect(askSentence(who("instance_admin", ["Root Admin"]))).toBe("An Aurora administrator can set this up.");
  });

  it("asks the role itself when no one is named, and never names an instance administrator", () => {
    render(<WhoCan value={who("workspace_owner", [])} />);
    expect(screen.getByText("Ask a workspace owner.")).toBeInTheDocument();
    cleanup();
    render(<WhoCan value={who("instance_admin", ["Root Admin"])} />);
    expect(screen.getByText("An Aurora administrator can set this up.")).toBeInTheDocument();
    expect(screen.queryByText(/Root Admin/)).toBeNull();
  });

  it("leads with the role and the action when asked (empty states, page headers)", () => {
    render(<WhoCan lead value={who("workspace_owner", ["Bob Builder"])} />);
    expect(screen.getByText("Only workspace owners can add an account. Ask a workspace owner.")).toBeInTheDocument();
    cleanup();
    render(<WhoCan lead value={who("instance_admin", [], { action: "set_up" })} />);
    expect(screen.getByText("Only an Aurora administrator can set this up.")).toBeInTheDocument();
  });

  it("shows the link instead of the sentence when the viewer can do it themselves", () => {
    render(<WhoCan lead value={who("workspace_owner", ["Bob Builder"], { you_can: true, path: "/o/acme/w/main/apps?tab=accounts" })} />);
    const link = screen.getByRole("link", { name: "Add an account" });
    expect(link.getAttribute("href")).toBe("/o/acme/w/main/apps?tab=accounts");
    expect(screen.queryByText(/Ask a workspace owner/)).toBeNull();
    expect(screen.queryByText(/Only workspace owners/)).toBeNull();
  });

  it("words an action key the dashboard has no phrase for as its own words", () => {
    expect(onlySentence({ role: "org_admin", action: "rotate_signing_key" })).toBe("Only organization owners and admins can rotate signing key.");
  });

  it("is built from the contacts of the place the viewer is in", () => {
    const contacts = { owners: [{ name: "Ada" }], admins: [{ name: "Alan" }, { name: "Grace" }] };
    expect(whoCanFromContacts("org_admin", contacts, "view_this").people).toEqual([{ name: "Alan" }, { name: "Grace" }]);
    expect(whoCanFromContacts("org_owner", contacts, "view_this").people).toEqual([{ name: "Ada" }]);
    expect(whoCanFromContacts("workspace_owner", { owners: [{ name: "Bob" }] }, "view_this").people).toEqual([{ name: "Bob" }]);
    expect(whoCanFromContacts("instance_admin", contacts, "view_this").people).toEqual([]);
    expect(whoCanFromContacts("workspace_owner", null, "view_this").people).toEqual([]);
    expect(orgRoleFor(["owner", "admin"])).toBe("org_admin");
    expect(orgRoleFor(["owner"])).toBe("org_owner");
  });

  it("MainAction shows the action to someone who can use it and who can in the same place to someone who cannot", () => {
    const value = who("workspace_owner", ["Bob Builder"]);
    render(
      <MainAction canAct whoCan={value}>
        <button type="button">Add an account</button>
      </MainAction>,
    );
    expect(screen.getByRole("button", { name: "Add an account" })).toBeInTheDocument();
    expect(screen.queryByText(/Only workspace owners/)).toBeNull();
    cleanup();
    render(
      <MainAction canAct={false} whoCan={value}>
        <button type="button">Add an account</button>
      </MainAction>,
    );
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("Only workspace owners can add an account. Ask a workspace owner.")).toBeInTheDocument();
  });
});

describe("refusals in words (C-915)", () => {
  const refusal = forbiddenError({ action: "view_this", role: "org_admin", people: [{ name: "Ada Lovelace" }, { name: "Alan Turing" }], you_can: false });

  it("a refused page names who can open it and never says the network is down", () => {
    render(<ErrorState error={refusal} />);
    expect(screen.getByText("Only organization owners and admins can open this page.")).toBeInTheDocument();
    expect(screen.getByText("Ask an organization admin.")).toBeInTheDocument();
    expect(screen.queryByText(/Could not reach the server/)).toBeNull();
  });

  it("the server's own 403 carries the same words", () => {
    const fromServer = new ApiError("forbidden", "m", { who_can: { action: "view_this", role: "workspace_owner", people: [{ name: "Bob Builder" }], you_can: false } }, 403);
    render(<ErrorState error={fromServer} />);
    expect(screen.getByText("Only workspace owners can open this page.")).toBeInTheDocument();
    expect(screen.getByText("Ask a workspace owner.")).toBeInTheDocument();
  });

  it("a 403 without who_can still reads as a refusal", () => {
    render(<ErrorState error={new ApiError("forbidden", "m", null, 403)} />);
    expect(screen.getByText(ERROR_MESSAGES.forbidden!)).toBeInTheDocument();
  });

  it("errorMessage maps ApiErrors, plain { code } objects and transport failures", () => {
    expect(errorMessage(new ApiError("forbidden", "m", null, 403))).toBe(ERROR_MESSAGES.forbidden);
    // A page gate's bare object is looked up like a server code, never read as "no answer".
    expect(errorMessage({ code: "forbidden" })).toBe(ERROR_MESSAGES.forbidden);
    expect(errorMessage({ code: "not_found" })).toBe(ERROR_MESSAGES.not_found);
    expect(errorMessage({ code: "unheard_of", message: "Custom words" })).toBe("Custom words");
    expect(errorMessage({ code: "unheard_of" })).toBe("Something went wrong.");
    // Only a missing answer is the network sentence.
    expect(errorMessage(new TypeError("Failed to fetch"))).toBe(NETWORK_SENTENCE);
    expect(errorMessage(new DOMException("aborted", "AbortError"))).toBe(NETWORK_SENTENCE);
    expect(errorMessage(new ApiError("unknown", "Unexpected error response", "<html>502</html>", 502))).toBe(NETWORK_SENTENCE);
    expect(errorMessage(new ApiError("network_error", "x", null, 0))).toBe(NETWORK_SENTENCE);
    expect(isTransportFailure(new ApiError("forbidden", "m", null, 403))).toBe(false);
    expect(isTransportFailure({ code: "forbidden" })).toBe(false);
  });

  it("has words for the connect and access refusals", () => {
    for (const code of ["connect_probe_failed", "oauth_client_required", "above_account_limit", "always_not_available"]) {
      expect(ERROR_MESSAGES[code], code).toBeTruthy();
    }
    expect(errorMessage(new ApiError("connect_probe_failed", "m", { cause: "rejected" }, 422))).toMatch(/did not accept this token/);
    expect(errorMessage(new ApiError("connect_probe_failed", "m", { cause: "unreachable", host: "jira.example.org" }, 422))).toMatch(/could not reach/);
    expect(errorMessage(new ApiError("connect_probe_failed", "m", { cause: "unexpected", upstream_status: 502 }, 422))).toMatch(/unexpected error/);
  });

  it("reads who_can only from a well-formed value", () => {
    expect(whoCanOf(refusal)?.role).toBe("org_admin");
    expect(whoCanOf(new ApiError("forbidden", "m", { who_can: { role: "nobody", action: "x", people: [] } }, 403))).toBeNull();
    expect(whoCanOf(new ApiError("forbidden", "m", null, 403))).toBeNull();
    expect(whoCanOf("nope")).toBeNull();
  });
});

describe("empty states (C-916)", () => {
  const owners = who("workspace_owner", ["Bob Builder"], { action: "add_schedule" });

  it("show who can only to a viewer who cannot act, and the action only to one who can", () => {
    render(
      <EmptyState title="No schedules" action={<button type="button">New schedule</button>} ask={{ whoCan: owners, canAct: true }}>
        A schedule wakes the agent.
      </EmptyState>,
    );
    expect(screen.getByRole("button", { name: "New schedule" })).toBeInTheDocument();
    expect(screen.queryByText(/Ask a workspace owner/)).toBeNull();
    cleanup();
    render(
      <EmptyState title="No schedules" action={<button type="button">New schedule</button>} ask={{ whoCan: owners, canAct: false }}>
        A schedule wakes the agent.
      </EmptyState>,
    );
    // Never both: a viewer who cannot act gets the sentence and no dead button.
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("Only workspace owners can set up a schedule. Ask a workspace owner.")).toBeInTheDocument();
  });

  it("cannot be given an ask without canAct (enforced by the type)", () => {
    // @ts-expect-error `ask` needs `canAct`
    const missing = <EmptyState title="x" ask={{ whoCan: owners }} />;
    // @ts-expect-error `ask` is a WhoCan with canAct, not a sentence
    const sentence = <EmptyState title="x" ask="Ask a workspace owner." />;
    expect(missing).toBeTruthy();
    expect(sentence).toBeTruthy();
  });
});

describe("the Welcome links (P5-UX-1)", () => {
  /** Does `href` name a page of the app? `[param]` directories match any segment. */
  function pageExists(href: string): boolean {
    const path = href.split("?")[0]!.split("#")[0]!;
    let dir = join(__dirname, "..", "src", "app");
    for (const seg of path.split("/").filter(Boolean)) {
      const names = readdirSync(dir, { withFileTypes: true }).filter((d) => d.isDirectory()).map((d) => d.name);
      const next = names.includes(seg) ? seg : names.find((n) => /^\[[^\]]+\]$/.test(n));
      if (!next) return false;
      dir = join(dir, next);
    }
    return existsSync(join(dir, "page.tsx"));
  }

  it("every link resolves to an existing route, for an owner and for a member", () => {
    for (const isOwner of [true, false]) {
      const links = optionalNextLinks({ org: "acme", ws: "main", handle: "daneel", agentName: "Daneel", isOwner });
      expect(links.length).toBeGreaterThan(0);
      for (const l of links) expect(pageExists(l.href), `${l.label} -> ${l.href}`).toBe(true);
    }
  });

  it("offers Give <Agent> an app (the connect journey for this agent, at its App step) to an owner, and no teammate invitation", () => {
    const owner = optionalNextLinks({ org: "acme", ws: "main", handle: "daneel", agentName: "Daneel", isOwner: true });
    const give = owner.find((l) => l.label === "Give Daneel an app");
    expect(give?.href).toBe("/o/acme/w/main/agents/daneel?tab=access&connect=");
    expect(owner.some((l) => /Invite|Connect an app/.test(l.label))).toBe(false);
    // A member cannot give an agent an app: the link is not offered (it would lead to a refusal).
    const member = optionalNextLinks({ org: "acme", ws: "main", handle: "daneel", agentName: "Daneel", isOwner: false });
    expect(member.some((l) => l.label.startsWith("Give "))).toBe(false);
  });
});

describe("first-run fixes (P5-UX-1)", () => {
  it("a label says (optional) exactly once", () => {
    expect(fieldLabel("Project", true)).toBe("Project (optional)");
    expect(fieldLabel("Project (optional)", true)).toBe("Project (optional)");
    expect(fieldLabel("Project (Optional)", true)).toBe("Project (optional)");
    expect(fieldLabel("Project (optional)", false)).toBe("Project");
    expect(fieldLabel("Project", false)).toBe("Project");
  });

  it("a help link reads 'Where do I find this?' and never prints the address", () => {
    render(
      <FieldControl
        f={{ name: "token", label: "Token (optional)", type: "secret", required: false, help_url: "https://example.org/settings/tokens" } as FormField}
        value=""
        onChange={() => undefined}
      />,
    );
    const link = screen.getByRole("link", { name: "Where do I find this?" });
    expect(link.getAttribute("href")).toBe("https://example.org/settings/tokens");
    expect(link.textContent).not.toContain("example.org");
    // And the optional label arrived once, though the manifest's own label already carried it.
    expect(screen.getByLabelText("Token (optional)")).toBeInTheDocument();
    expect(screen.queryByText(/\(optional\) \(optional\)/)).toBeNull();
  });

  it("a tab strip scrolls without drawing a scrollbar widget", () => {
    render(
      <Tabs
        label="Sections"
        value="a"
        onValueChange={() => undefined}
        variant="underline"
        items={[
          { value: "a", label: "Alpha" },
          { value: "b", label: "Beta" },
        ]}
      />,
    );
    const strip = screen.getByRole("tablist", { name: "Sections" });
    expect(strip.className).toContain("[scrollbar-width:none]");
    expect(strip.className).toContain("[&::-webkit-scrollbar]:hidden");
  });
});
