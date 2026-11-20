import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AddPeople, WaitingList, addError, daysLeft, resultSentence } from "@/components/settings/add-people";
import { optionalNextLinks } from "@/components/onboarding/getting-started";
import { ApiError } from "@/lib/api/client";
import { json } from "./support/api";

// (P5-UX-5, C-919, C-920, `14 §10`) Add people: an e-mail and a role, the one-sentence result, the link to send, the
// organization's admins to ask when an invitation is needed, the roles offered, and the waiting list.

type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
let calls: { key: string; body: unknown }[];
let routes: Record<string, Handler>;

beforeEach(() => {
  calls = [];
  routes = {
    "GET /v1/orgs/o1": () => json(200, { id: "o1", slug: "acme", name: "Acme", contacts: { admins: [{ name: "Ada Lovelace" }, { name: "Alan Turing" }], owners: [] } }),
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const body = text ? JSON.parse(text) : undefined;
      const key = `${req.method} ${url.pathname}`;
      calls.push({ key, body });
      const h = routes[key];
      return h ? h(req, body) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});
const wrap = (ui: React.ReactElement) => render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);

const dialog = (props: Partial<React.ComponentProps<typeof AddPeople>> = {}) =>
  wrap(<AddPeople wsId="w1" orgId="o1" workspacePath="/o/acme/w/bob" hasApprover={false} initiallyOpen {...props} />);
const fill = (email: string) => fireEvent.change(screen.getByLabelText("E-mail"), { target: { value: email } });

describe("the words", () => {
  it("says the two results in one sentence each", () => {
    expect(resultSentence("ada@x.org", "member", { result: "added" })).toBe("ada@x.org can now use this workspace.");
    expect(resultSentence("carol@x.org", "member", { result: "pending" })).toBe("carol@x.org will have access as Member after they first sign in.");
    expect(resultSentence("carol@x.org", "approver", { result: "pending" })).toBe("carol@x.org will have access as Approver after they first sign in.");
  });

  it("says the refusals in words", () => {
    expect(addError(new ApiError("pending_access_limit", "x", null, 422), "Acme")).toBe("50 people are already waiting. Remove one first.");
    expect(addError(new ApiError("owner_requires_member", "x", null, 422), "Acme")).toMatch(/Only someone who is already in Acme can be an owner/);
  });

  it("counts a started day", () => {
    const now = Date.parse("2026-11-17T12:00:00Z");
    expect(daysLeft("2026-12-17T12:00:00Z", now)).toBe(30);
    expect(daysLeft("2026-11-18T00:00:00Z", now)).toBe(1);
    expect(daysLeft("2026-11-16T00:00:00Z", now)).toBe(0);
  });
});

describe("the Add people dialog", () => {
  it("offers Member and Owner, and Approver only in a workspace that already has one", async () => {
    const { unmount } = dialog();
    const group = await screen.findByRole("radiogroup", { name: "Role" });
    expect(within(group).getAllByRole("radio").map((r) => r.getAttribute("value"))).toEqual(["member", "owner"]);
    unmount();
    dialog({ hasApprover: true });
    const group2 = await screen.findByRole("radiogroup", { name: "Role" });
    expect(within(group2).getAllByRole("radio").map((r) => r.getAttribute("value"))).toEqual(["member", "approver", "owner"]);
  });

  it("someone not in the organization yet: waiting, the link to send, and who to ask for an invitation", async () => {
    routes["POST /v1/workspaces/w1/access/by-email"] = () => json(200, { result: "pending", org_entry: "invitation_needed" });
    dialog();
    fill("carol@example.org");
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(await screen.findByText("carol@example.org will have access as Member after they first sign in.")).toBeInTheDocument();
    expect(calls.find((c) => c.key === "POST /v1/workspaces/w1/access/by-email")?.body).toEqual({ email: "carol@example.org", role: "member" });
    expect(screen.getByText("Aurora does not e-mail them. Send them this link:")).toBeInTheDocument();
    expect(screen.getByText(`${window.location.origin}/o/acme/w/bob`)).toBeInTheDocument();
    expect(await screen.findByText(/They also need to be invited to Acme\. Ask an organization admin\./)).toBeInTheDocument();
  });

  it("open sign-up: no invitation sentence; someone already in the organization: access at once", async () => {
    routes["POST /v1/workspaces/w1/access/by-email"] = (_r, b) =>
      json(200, (b as { email: string }).email.startsWith("ada") ? { result: "added", org_entry: "member" } : { result: "pending", org_entry: "open_signup" });
    dialog();
    fill("dan@example.org");
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(await screen.findByText("dan@example.org will have access as Member after they first sign in.")).toBeInTheDocument();
    expect(screen.queryByText(/invited to Acme/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Add another" }));
    fill("ada@example.org");
    fireEvent.click(await screen.findByRole("button", { name: "Add" }));
    expect(await screen.findByText("ada@example.org can now use this workspace.")).toBeInTheDocument();
    expect(screen.getByText("Aurora does not e-mail them. Send them this link:")).toBeInTheDocument();
  });

  it("says in words why the server refused: owner for a non-member, and the maximum", async () => {
    routes["POST /v1/workspaces/w1/access/by-email"] = (_r, b) =>
      (b as { role: string }).role === "owner"
        ? json(422, { error: { code: "owner_requires_member", message: "m" } })
        : json(422, { error: { code: "pending_access_limit", message: "m" } });
    dialog();
    fill("x@example.org");
    fireEvent.click(screen.getByRole("radio", { name: "Owner" }));
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/Only someone who is already in Acme can be an owner/);
    fireEvent.click(screen.getByRole("radio", { name: "Member" }));
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("50 people are already waiting. Remove one first."));
  });

  it("does not send an empty address", () => {
    dialog();
    expect(screen.getByRole("button", { name: "Add" })).toBeDisabled();
  });
});

describe("Waiting for first sign-in", () => {
  const soon = new Date(Date.now() + 10 * 86_400_000 + 3_600_000).toISOString();
  const item = (id: string, email: string, role = "member") => ({ id, email, role, expires_at: soon, created_at: soon });

  it("lists each person with role and expiry, and Remove deletes the row", async () => {
    let items = [item("p1", "carol@example.org"), item("p2", "dan@example.org", "approver")];
    routes["GET /v1/workspaces/w1/access/pending"] = () => json(200, { items, next_cursor: null });
    routes["DELETE /v1/workspaces/w1/access/pending/p1"] = () => {
      items = items.filter((i) => i.id !== "p1");
      return json(204, null);
    };
    wrap(<WaitingList wsId="w1" orgId="o1" />);
    expect(await screen.findByText("carol@example.org · Member · expires in 11 days")).toBeInTheDocument();
    expect(screen.getByText("dan@example.org · Approver · expires in 11 days")).toBeInTheDocument();
    expect(await screen.findByText(/Access starts when they first sign in to Acme with this verified e-mail\./)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Remove carol@example.org" }));
    await waitFor(() => expect(screen.queryByText(/carol@example\.org ·/)).toBeNull());
    expect(calls.some((c) => c.key === "DELETE /v1/workspaces/w1/access/pending/p1")).toBe(true);
  });

  it("is not shown when empty", async () => {
    routes["GET /v1/workspaces/w1/access/pending"] = () => json(200, { items: [], next_cursor: null });
    const { container } = wrap(<WaitingList wsId="w1" orgId="o1" />);
    await waitFor(() => expect(calls.some((c) => c.key === "GET /v1/workspaces/w1/access/pending")).toBe(true));
    expect(container.textContent).toBe("");
  });
});

describe("Welcome's Add people link", () => {
  it("is offered to a workspace owner and opens the dialog; a member does not see it", () => {
    const owner = optionalNextLinks({ org: "acme", ws: "main", handle: "daneel", agentName: "Daneel", isOwner: true });
    expect(owner.find((l) => l.label === "Add people")?.href).toBe("/o/acme/w/main/people?add=1");
    const member = optionalNextLinks({ org: "acme", ws: "main", handle: "daneel", agentName: "Daneel", isOwner: false });
    expect(member.some((l) => l.label === "Add people")).toBe(false);
  });
});
