import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { PeersTab } from "@/components/peers/peers-tab";
import { json } from "./support/api";

const err = (status: number, code: string, message = code, details?: unknown) => json(status, { error: { code, message, details } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${decodeURIComponent(url.pathname)}`];
    return h ? h(req) : err(404, "not_found", "nf");
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
let qcRef: QueryClient;
function wrap(ui: React.ReactNode) {
  qcRef = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qcRef}>{ui}</QueryClientProvider>);
}

const link = {
  id: "l1", revision: 3, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
  self: { agent_id: "a1", policy: "ask" },
  peer: { agent_id: "a2", handle: "bob", name: "Bob", workspace_id: "w2", workspace_name: "Research", policy: "allow" },
  direction: "both", created_by: { kind: "agent", display_name: null },
};

const peersProps = { agentId: "a1", agentName: "Daneel", agentsHref: "/o/acme/w/main/agents", wsId: "w1" };

describe("PeersTab", () => {
  it("lists links and tasks; policy change sends revision; 409 refetches", async () => {
    let body: unknown = null;
    let linkFetches = 0;
    mockFetch({
      "GET /v1/agents/a1/links": () => {
        linkFetches++;
        return json(200, { items: [link] });
      },
      "GET /v1/agents/a1/peer-tasks": () =>
        json(200, {
          items: [{
            id: "t1", link_id: "l1", from_agent_id: "a1", to_agent_id: "a2", direction: "outgoing", origin_conversation_id: "c1",
            task_conversation_id: "c2", status: "completed", messages_used: 2, message_budget: 10, report: "All done",
            expires_at: "2026-02-01T00:00:00Z", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
          }],
          next_cursor: null,
        }),
      "PUT /v1/agent-links/l1/policy": async (r) => {
        body = await r.json();
        return err(409, "stale_revision", "stale");
      },
    });
    wrap(<PeersTab {...peersProps} />);
    expect(await screen.findByText("Bob")).toBeTruthy();
    expect(await screen.findByText("All done")).toBeTruthy();
    expect(screen.getByText("Daneel and Bob can each give the other tasks.")).toBeTruthy();
    expect(screen.getByText(/Opened by Daneel · Bob allows it/)).toBeTruthy();
    expect(screen.getByText(/Daneel handed a task to Bob · completed ·/)).toBeTruthy();
    expect(screen.queryByText(/Budget/)).toBeNull();
    fireEvent.change(screen.getByLabelText("My policy for Bob"), { target: { value: "blocked" } });
    await waitFor(() => expect(body).toEqual({ agent_id: "a1", policy: "blocked", revision: 3 }));
    expect((await screen.findByRole("alert")).textContent).toContain("Someone changed this link. The latest version is loaded; try again.");
    await waitFor(() => expect(linkFetches).toBe(2));
  });

  it("a member cannot choose Allowed (only an approver can allow this)", async () => {
    mockFetch({
      "GET /v1/agents/a1/links": () => json(200, { items: [link] }),
      "GET /v1/agents/a1/peer-tasks": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/workspaces/w1": () => json(200, { id: "w1", my_role: "member" }),
    });
    wrap(<PeersTab {...peersProps} />);
    const select = (await screen.findByLabelText("My policy for Bob")) as HTMLSelectElement;
    const allow = Array.from(select.options).find((o) => o.value === "allow")!;
    await waitFor(() => expect(allow.disabled).toBe(true));
    expect(allow.title).toBe("Only an approver can allow this");
    expect(Array.from(select.options).map((o) => o.textContent)).toEqual(["Ask me first", "Allowed (only an approver can allow this)", "Blocked"]);
  });

  it("empty: says so, and Link another agent creates the link with the chosen direction", async () => {
    let created: unknown = null;
    mockFetch({
      "GET /v1/agents/a1/links": () => json(200, { items: [] }),
      "GET /v1/agents/a1/peer-tasks": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/agents/a1/linkable": () =>
        json(200, {
          items: [
            { agent_id: "a2", name: "Rex", handle: "rex", workspace_id: "w2", workspace_name: "Ops", linked: false, hosted: true, frozen: false },
            { agent_id: "a3", name: "Old", handle: "old", workspace_id: "w2", workspace_name: "Ops", linked: true, hosted: true, frozen: false },
            { agent_id: "a4", name: "Sleepy", handle: "sleepy", workspace_id: "w2", workspace_name: "Ops", linked: false, hosted: true, frozen: true },
            { agent_id: "a5", name: "Own", handle: "own", workspace_id: "w2", workspace_name: "Ops", linked: false, hosted: false, frozen: false },
          ],
        }),
      "POST /v1/agents/a1/links": async (r) => {
        created = await r.json();
        return json(201, { ...link, id: "l9", peer: { ...link.peer, agent_id: "a2", name: "Rex" } });
      },
    });
    wrap(<PeersTab {...peersProps} />);
    expect(await screen.findByText("Daneel isn't linked to another agent")).toBeTruthy();
    expect(screen.getByText("Linked agents can hand each other tasks. The tasks they exchange appear here.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Link another agent" }));
    const dialog = await screen.findByRole("dialog");
    const picker = (await within(dialog).findByLabelText("Agent")) as HTMLSelectElement;
    const text = Array.from(picker.options).map((o) => [o.textContent, o.disabled]);
    expect(text).toContainEqual(["Rex · Ops", false]);
    expect(text).toContainEqual(["Old · Ops (Already linked)", true]);
    expect(text).toContainEqual(["Sleepy · Ops (Paused)", true]);
    expect(text).toContainEqual(["Own · Ops (Runs on its own computer)", true]);
    expect((within(dialog).getByRole("button", { name: "Link agents" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(picker, { target: { value: "a2" } });
    expect(within(dialog).getByText("Both ways")).toBeTruthy();
    expect(within(dialog).getByText("Daneel can hand tasks to Rex")).toBeTruthy();
    expect(within(dialog).getByText("Daneel only works on what Rex sends.")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("radio", { name: /Daneel can hand tasks to Rex/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Link agents" }));
    await waitFor(() => expect(created).toEqual({ peer_agent_id: "a2", direction: "to_peer" }));
  });

  it("an empty linkable list says there is no other agent and links to Agents", async () => {
    mockFetch({
      "GET /v1/agents/a1/links": () => json(200, { items: [] }),
      "GET /v1/agents/a1/peer-tasks": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/agents/a1/linkable": () => json(200, { items: [] }),
    });
    wrap(<PeersTab {...peersProps} />);
    fireEvent.click(await screen.findByRole("button", { name: "Link another agent" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(/There is no other agent you can link\./)).toBeTruthy();
    expect(within(dialog).getByRole("link", { name: "Create one first." }).getAttribute("href")).toBe("/o/acme/w/main/agents");
  });

  it("Remove link asks first, then deletes; Change direction sends the revision", async () => {
    let direction: unknown = null;
    let deleted = false;
    mockFetch({
      "GET /v1/agents/a1/links": () => json(200, { items: [link] }),
      "GET /v1/agents/a1/peer-tasks": () => json(200, { items: [], next_cursor: null }),
      "PUT /v1/agent-links/l1/direction": async (r) => {
        direction = await r.json();
        return json(200, { ...link, direction: "outgoing" });
      },
      "DELETE /v1/agent-links/l1": () => {
        deleted = true;
        return new Response(null, { status: 204 });
      },
    });
    wrap(<PeersTab {...peersProps} />);
    const open = async (name: string) => {
      fireEvent.keyDown(await screen.findByRole("button", { name: "Actions for the link with Bob" }), { key: "Enter" });
      fireEvent.click(await screen.findByRole("menuitem", { name }));
    };
    await open("Change direction…");
    let dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("radio", { name: /Daneel can hand tasks to Bob/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save direction" }));
    await waitFor(() => expect(direction).toEqual({ agent_id: "a1", direction: "to_peer", revision: 3 }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await open("Remove link…");
    dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Daneel and Bob will stop handing each other tasks. Tasks in progress between them are cancelled.")).toBeTruthy();
    expect(deleted).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove link" }));
    await waitFor(() => expect(deleted).toBe(true));
  });
});
