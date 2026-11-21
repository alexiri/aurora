import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AgentState } from "@/components/agents/agent-state";
import { AnswerFixAction } from "@/components/agents/answer-fix";
import { ChatTab } from "@/components/agents/chat-tab";
import { resetStopped } from "@/lib/chat/stopped";
import { json } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-11 item 8 and P5-REFINE-12 item 7 (D-414, access-3): an agent with no AI key says "<Agent> has no AI model yet" in its
// empty chat (the composer is off) with the one action chosen for the viewer, and the header's "Can't answer: no AI key" carries
// the same action: give the Company key in one click, add a key (organization admins) or ask an admin, with the optional reason.

const st = vi.hoisted(() => ({ events: [] as unknown[] }));
vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({
    state: { events: st.events, deltas: {}, activity: {}, purged: {}, lastSeq: st.events.length },
    loading: false,
    error: null,
    live: true,
  }),
}));

const chat = {
  id: "d1", agent_id: "a1", kind: "direct", external_ref: "d1", title: null, title_source: "auto", direct_user_id: null, direct_user: null, origin_conversation_id: null,
  last_seq: 0, app_turn_streak: 0, app_turn_capped: false, active_turn: null, last_activity_at: null, archived_at: null, automation: null, run_at: null,
  created_at: "2026-01-01T00:00:00Z",
};
type Fix =
  | { action: "give_key"; key: { secret_id: string; name: string; scope: string } }
  | { action: "add_key" }
  | { action: "ask_admin"; who_can: unknown };
const NO_KEY = (fix: Fix | null) => ({ can_answer: false, reason: "no_ai_key", usable_keys: 0, fix });
const CAN = { can_answer: true, reason: null, usable_keys: 1, fix: null };
const ASK: Fix = { action: "ask_admin", who_can: { roles: ["organization owner", "organization admin"] } };
const GIVE: Fix = { action: "give_key", key: { secret_id: "s1", name: "Company key", scope: "organization" } };

interface Call { method: string; path: string; body: unknown }
let calls: Call[];
let readiness: unknown;
let requests: unknown[];

beforeEach(() => {
  st.events = [];
  calls = [];
  requests = [];
  window.history.replaceState(null, "", "/");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      calls.push({ method: req.method, path: url.pathname, body: text ? JSON.parse(text) : undefined });
      const key = `${req.method} ${url.pathname}`;
      if (key === "GET /v1/agents/a1/answer-readiness") return json(200, readiness);
      if (key === "POST /v1/agents/a1/conversations/direct" || key === "GET /v1/agents/a1/conversations/direct") return json(200, chat);
      if (key === "GET /v1/agents/a1/conversations") return json(200, { items: [chat], next_cursor: null });
      if (key === "GET /v1/agents/a1/conversation-folders") return json(200, { items: [], next_cursor: null });
      if (key === "POST /v1/secrets/s1/grants") {
        readiness = CAN;
        return json(200, { items: [{ agent_id: "a1", status: "created" }] });
      }
      if (key === "GET /v1/workspaces/w/requests") return json(200, { items: requests });
      if (key === "POST /v1/workspaces/w/requests") {
        requests = [{ id: "r1", subject: "ai_key", connector_type: null }];
        return json(201, { id: "r1", subject: "ai_key" });
      }
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});
afterEach(() => {
  resetStopped();
  vi.unstubAllGlobals();
});

const tab = () => wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" agent={{ id: "a1", name: "Daneel", handle: "daneel" }} />);

describe("the empty chat of an agent with no AI key", () => {
  it("says it has no AI model yet, turns the composer off, and asks an admin with an optional reason", async () => {
    readiness = NO_KEY(ASK);
    tab();
    const card = await screen.findByRole("region", { name: "Daneel has no AI model yet" });
    expect(within(card).getByRole("heading", { name: "Daneel has no AI model yet" })).toBeInTheDocument();
    expect(card).toHaveTextContent("Only organization owners and admins can add an AI key.");
    expect(screen.queryByText("Say hello to Daneel")).toBeNull();
    expect(screen.queryByLabelText("Message", { exact: true })).toBeNull();
    expect(screen.getByTestId("no-model-bar")).toHaveTextContent("Daneel has no AI model yet, so it can't answer.");
    // Ask an admin, with the reason of C-1081.
    expect(within(card).getByText("Why do you need it?")).toBeInTheDocument();
    fireEvent.change(within(card).getByLabelText(/Why do you need it/), { target: { value: "So Daneel can answer in #ops" } });
    fireEvent.click(within(card).getByRole("button", { name: "Send request" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST" && c.path === "/v1/workspaces/w/requests")?.body).toEqual({ subject: "ai_key", reason: "So Daneel can answer in #ops" }));
    expect(await screen.findByText("Asked an admin")).toBeInTheDocument();
  });

  it("an organization admin gets Add a key, a link to the organization's AI keys", async () => {
    readiness = NO_KEY({ action: "add_key" });
    tab();
    const card = await screen.findByRole("region", { name: "Daneel has no AI model yet" });
    expect(within(card).getByRole("link", { name: "Add a key" }).getAttribute("href")).toBe("/o/x/apps?tab=accounts&kind=ai-key");
    expect(card).toHaveTextContent("Daneel can't answer until you add an AI key.");
    expect(screen.queryByLabelText("Message", { exact: true })).toBeNull();
  });

  it("an owner can give the organization's key in one click, and the chat opens up at once", async () => {
    readiness = NO_KEY(GIVE);
    tab();
    const card = await screen.findByRole("region", { name: "Daneel has no AI model yet" });
    fireEvent.click(within(card).getByRole("button", { name: "Give Daneel the Company key" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST" && c.path === "/v1/secrets/s1/grants")?.body).toEqual({ agent_ids: ["a1"] }));
    expect(await screen.findByText("Say hello to Daneel")).toBeInTheDocument();
    expect(await screen.findByLabelText("Message", { exact: true })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Daneel has no AI model yet" })).toBeNull();
  });

  it("an agent that can answer keeps its hello and its composer", async () => {
    readiness = CAN;
    tab();
    expect(await screen.findByText("Say hello to Daneel")).toBeInTheDocument();
    expect(await screen.findByLabelText("Message", { exact: true })).toBeInTheDocument();
    expect(screen.queryByTestId("no-model-card")).toBeNull();
    expect(screen.queryByTestId("no-model-bar")).toBeNull();
  });

  it("a key that exists but cannot be used is not 'no AI model yet' (the header says why)", async () => {
    readiness = { can_answer: false, reason: "key_not_usable", usable_keys: 0, fix: ASK };
    tab();
    expect(await screen.findByText("Say hello to Daneel")).toBeInTheDocument();
    expect(screen.queryByTestId("no-model-card")).toBeNull();
  });
});

describe("the header's action (access-3)", () => {
  // The page header puts the action on its own row after the state line.
  const header = () =>
    wrap(
      <>
        <AgentState agentId="a1" wsId="w" canApprove={false} hosted />
        <AnswerFixAction agentId="a1" agentName="Daneel" wsId="w" base="/o/x/w/y" />
      </>,
    );

  it("'Can't answer: no AI key' carries Give Daneel the Company key; one click, a toast, and the header turns Ready", async () => {
    readiness = NO_KEY(GIVE);
    header();
    expect(await screen.findByText("Can't answer: no AI key")).toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "Give Daneel the Company key" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/v1/secrets/s1/grants")).toBe(true));
    expect(await screen.findByText("Ready")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Give Daneel/ })).toBeNull();
  });

  it("a member who cannot give it sees Ask an admin, which opens the optional reason", async () => {
    readiness = NO_KEY(ASK);
    header();
    fireEvent.click(await screen.findByRole("button", { name: "Ask an admin" }));
    fireEvent.change(await screen.findByLabelText(/Why do you need it/), { target: { value: "Needed for the report" } });
    fireEvent.click(screen.getByRole("button", { name: "Send request" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST" && c.path === "/v1/workspaces/w/requests")?.body).toEqual({ subject: "ai_key", reason: "Needed for the report" }));
  });

  it("an organization admin gets Add a key; an agent that can answer has no action", async () => {
    readiness = NO_KEY({ action: "add_key" });
    const { unmount } = header();
    expect((await screen.findByRole("link", { name: "Add a key" })).getAttribute("href")).toBe("/o/x/apps?tab=accounts&kind=ai-key");
    unmount();
    readiness = CAN;
    header();
    expect(await screen.findByText("Ready")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Add a key" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Ask an admin" })).toBeNull();
  });
});
