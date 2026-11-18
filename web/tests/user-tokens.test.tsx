import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { err, json } from "./support/api";

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(""),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useParams: () => ({}),
  usePathname: () => "/",
}));

import { SEND_AS_ME_HELP } from "@/components/account/send-as-me";
import { ApprovalsView } from "@/components/approvals/approvals-view";
import { AccountChannelsView } from "@/components/channels/account-channels-view";

// (P4-UTOK, D-281..D-284, C-853..C-856) Account › Send as me and the owner's "Posts as you" on the Approvals page.

type Handler = (req: Request, body: unknown) => Response | Promise<Response>;
interface Rec {
  key: string;
  body: unknown;
}
function mockFetch(routes: Record<string, Handler>) {
  const calls: Rec[] = [];
  vi.stubGlobal(
    "EventSource",
    class {
      addEventListener() {}
      close() {}
    },
  );
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      const key = `${req.method} ${url.pathname}`;
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const body = text ? (JSON.parse(text) as unknown) : null;
      calls.push({ key, body });
      const h = routes[key];
      return h ? h(req, body) : err(404, "not_found");
    }),
  );
  return calls;
}

const T = "2026-01-01T00:00:00Z";
const me = {
  user: { id: "u1", display_name: "Ada", email: "a@x.test" },
  memberships: [
    { org: { id: "o1", name: "Acme", slug: "acme" }, role: "member", status: "active", workspaces: [{ id: "w1", name: "Main", slug: "main", my_role: "member" }] },
  ],
};
const link = {
  id: "l1", integration_id: "i1", connector_id: "mm", tenant_label: "Acme Chat", platform_user_ref: "U123",
  user: { id: "u1", display_name: "Ada" }, method: "admin", created_at: T,
};
const token = (over: Record<string, unknown> = {}) => ({
  id: "t1", integration: { id: "i1", type: "mm" }, label: "work", platform_user_label: "ada", status: "active",
  agents: [], created_at: T, last_used_at: null, ...over,
});

function base(extra: Record<string, Handler>) {
  return mockFetch({
    "GET /v1/me": () => json(200, me),
    "GET /v1/workspaces/w1/agents": () =>
      json(200, { items: [{ id: "a1", name: "Helper", handle: "helper" }, { id: "a2", name: "Scribe", handle: "scribe" }], next_cursor: null }),
    "GET /v1/me/channel-links": () => json(200, { items: [link] }),
    "GET /v1/me/dm-defaults": () => json(200, { items: [] }),
    ...extra,
  });
}

describe("Send as me", () => {
  it("shows the empty state and the help text, and never echoes the token", async () => {
    base({ "GET /v1/me/user-tokens": () => json(200, { tokens: [] }) });
    wrap(<AccountChannelsView />);
    expect(await screen.findByText("Agents post as themselves unless you add a token.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add token" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(SEND_AS_ME_HELP)).toBeInTheDocument();
    const field = within(dialog).getByLabelText("Personal access token") as HTMLInputElement;
    expect(field.type).toBe("password");
    expect(field.autocomplete).toBe("off");
    fireEvent.change(field, { target: { value: "pat-secret-123" } });
    // Masked: nothing on the page shows the value as text.
    expect(screen.queryByText("pat-secret-123")).toBeNull();
  });

  it("adds a token: the body carries the value once, and a success closes the dialog", async () => {
    let listed = false;
    const calls = base({
      "GET /v1/me/user-tokens": () => json(200, { tokens: listed ? [token()] : [] }),
      "POST /v1/me/user-tokens": () => {
        listed = true;
        return json(201, token());
      },
    });
    wrap(<AccountChannelsView />);
    fireEvent.click(await screen.findByRole("button", { name: "Add token" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Personal access token"), { target: { value: "pat-1" } });
    fireEvent.change(within(dialog).getByLabelText("Name (optional)"), { target: { value: "work" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add token" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(calls.find((c) => c.key === "POST /v1/me/user-tokens")?.body).toEqual({ integration_id: "i1", token: "pat-1", label: "work" });
    expect(await screen.findByText("ada")).toBeInTheDocument();
    expect(JSON.stringify(document.body.innerHTML)).not.toContain("pat-1");
  });

  it.each([
    [422, "user_token_mismatch", "belongs to another account"],
    [422, "user_token_invalid", "rejected this token"],
    [409, "user_token_limit", "at most 5 tokens"],
    [503, "secret_store_unavailable", "nothing was saved"],
  ])("maps %s %s inline and clears the typed value", async (status, code, text) => {
    base({
      "GET /v1/me/user-tokens": () => json(200, { tokens: [] }),
      "POST /v1/me/user-tokens": () => err(status, code, "<b>raw</b>"),
    });
    wrap(<AccountChannelsView />);
    fireEvent.click(await screen.findByRole("button", { name: "Add token" }));
    const dialog = await screen.findByRole("dialog");
    const field = within(dialog).getByLabelText("Personal access token") as HTMLInputElement;
    fireEvent.change(field, { target: { value: "pat-x" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add token" }));
    await waitFor(() => expect(dialog.textContent).toContain(text));
    expect(dialog.querySelector("b")).toBeNull(); // plain text, never markup
    expect(field.value).toBe("");
  });

  it("lists tokens with status and agents, grants and removes agents with switches", async () => {
    let t = token();
    const calls = base({
      "GET /v1/me/user-tokens": () => json(200, { tokens: [t] }),
      "PUT /v1/me/user-tokens/t1/agents/a1": () => {
        t = token({ agents: [{ id: "a1", name: "Helper" }] });
        return json(204, null);
      },
      "DELETE /v1/me/user-tokens/t1/agents/a1": () => {
        t = token();
        return json(204, null);
      },
    });
    wrap(<AccountChannelsView />);
    const list = await screen.findByRole("list", { name: "Send as me tokens" });
    expect(within(list).getByText("active")).toBeInTheDocument();
    expect(within(list).getByText("None yet")).toBeInTheDocument();
    fireEvent.click(within(list).getByRole("button", { name: "Grant agents for work" }));
    const dialog = await screen.findByRole("dialog");
    const helper = (await within(dialog).findByLabelText("Helper (@helper) · Main")) as HTMLInputElement;
    expect(helper.checked).toBe(false);
    fireEvent.click(helper);
    await waitFor(() => expect(calls.some((c) => c.key === "PUT /v1/me/user-tokens/t1/agents/a1")).toBe(true));
    await waitFor(() => expect((within(dialog).getByLabelText("Helper (@helper) · Main") as HTMLInputElement).checked).toBe(true));
    fireEvent.click(within(dialog).getByLabelText("Helper (@helper) · Main"));
    await waitFor(() => expect(calls.some((c) => c.key === "DELETE /v1/me/user-tokens/t1/agents/a1")).toBe(true));
  });

  it("an invalid token says so and cannot be granted; revoke asks first", async () => {
    const calls = base({
      "GET /v1/me/user-tokens": () => json(200, { tokens: [token({ status: "invalid" })] }),
      "DELETE /v1/me/user-tokens/t1": () => json(204, null),
    });
    wrap(<AccountChannelsView />);
    expect(await screen.findByText(/rejected this token/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Grant agents for work" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Revoke work" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Agents stop posting as you and pending posts are cancelled.");
    expect(calls.some((c) => c.key === "DELETE /v1/me/user-tokens/t1")).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Revoke token" }));
    await waitFor(() => expect(calls.some((c) => c.key === "DELETE /v1/me/user-tokens/t1")).toBe(true));
  });

  it("without a linked chat account there is nothing to add yet", async () => {
    base({
      "GET /v1/me/channel-links": () => json(200, { items: [] }),
      "GET /v1/me/user-tokens": () => json(200, { tokens: [] }),
    });
    wrap(<AccountChannelsView />);
    expect(await screen.findByText(/Link a chat account first/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add token" })).toBeNull();
  });
});

describe("Posts as you (Approvals)", () => {
  const send = {
    id: "s1", workspace_id: "w1", agent_id: "a1", token_id: "t1",
    summary: "Post as Ada in General (13 characters)", text: "Hello <b>team</b>\nsecond line", status: "pending",
    error: null, expires_at: "2099-01-01T00:00:00Z", created_at: T,
  };
  const heldByOwner = {
    id: "s1", workspace_id: "w1", agent_id: "a1", conversation_id: null, turn_id: null, conversation_hidden: false,
    action_key: "send_as_me", summary: "Post as Ada in General (13 characters)", status: "pending", decided_by: null,
    decided_at: null, executed_at: null, error: null, expires_at: "2099-01-01T00:00:00Z", created_at: T,
  };

  it("shows the owner the full text as plain text and decides with their own controls", async () => {
    let items: unknown[] = [send];
    const calls = mockFetch({
      "GET /v1/workspaces/w1/approvals": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/workspaces/w1/action-approvals": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/me/user-token-approvals": () => json(200, { items }),
      "POST /v1/action-approvals/s1/decision": () => {
        items = [];
        return json(200, { ...heldByOwner, status: "executed" });
      },
    });
    wrap(<ApprovalsView wsId="w1" />);
    const section = await screen.findByRole("region", { name: "Posts as you" });
    const quote = within(section).getByText(/Hello/);
    expect(quote.textContent).toBe("Hello <b>team</b>\nsecond line");
    expect(quote.querySelector("b")).toBeNull();
    expect(within(section).getByText("Post as Ada in General (13 characters)")).toBeInTheDocument();
    fireEvent.click(within(section).getByRole("button", { name: "Post as me" }));
    await waitFor(() => expect(calls.find((c) => c.key === "POST /v1/action-approvals/s1/decision")?.body).toEqual({ decision: "approve" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Posts as you" })).toBeNull());
  });

  it("shows nothing when nobody proposed a post, and no decide control to anyone else", async () => {
    mockFetch({
      "GET /v1/workspaces/w1/approvals": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/me/user-token-approvals": () => json(200, { items: [] }),
      "GET /v1/workspaces/w1/action-approvals": () => json(200, { items: [heldByOwner], next_cursor: null }),
    });
    wrap(<ApprovalsView wsId="w1" />);
    expect(await screen.findByText("Only the person it would speak for can decide this.")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Posts as you" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Allow once" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Block" })).toBeNull();
  });
});
