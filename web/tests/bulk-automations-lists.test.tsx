import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AutomationsTab } from "@/components/automations/automations-tab";
import { UndoToastHost } from "@/components/ui/undo-toast";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-19 item 3, Automations: the bar and the checkboxes are on the Automations page's one list, for schedules and
// webhooks (a chat row has none), for owners, with Show more controls on.

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const sched = (id: string, name: string, enabled = true) => ({
  id, agent_id: "a1", name, prompt: "Do it.", cron: "0 8 * * *", timezone: "UTC", enabled, consecutive_failures: 0, anchor: "creator",
  conversation_hidden: false, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z", next_runs: [], created_by: { id: "u", display_name: "U" },
});
const hook = (id: string, name: string, enabled = true) => ({
  id, agent_id: "a1", name, instructions: "Say.", enabled, url_prefix: "https://a.test/hooks/x", verification: { kind: "none" }, anchor: "creator",
  created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z", created_by: { id: "u", display_name: "U" }, last_delivery_at: null,
});
const chat = {
  id: "p1", agent_id: "a1", connector_id: "c1", tenant_label: "acme", mode: "bot", status: "active", status_reason: null,
};

const page = (canOwn: boolean) => <AutomationsTab agentId="a1" agentName="Daneel" agentBase="/o/acme/w/main/agents/daneel" wsId="w1" canOwn={canOwn} />;
const routes = (extra: Record<string, () => Response> = {}) => ({
  "GET /v1/agents/a1/schedules": () => json(200, { items: [sched("s1", "Morning"), sched("s2", "Evening", false)], next_cursor: null }),
  "GET /v1/agents/a1/webhooks": () => json(200, { items: [hook("h1", "Deploy"), hook("h2", "Alerts")], next_cursor: null }),
  "GET /v1/agents/a1/presences": () => json(200, { items: [chat], next_cursor: null }),
  ...extra,
});

describe("Automations list", () => {
  it("an owner at Expert can tick schedules and pause them", async () => {
    mockFetch(routes({ "PATCH /v1/schedules/s1": () => json(200, {}) }));
    wrap(<>{page(true)}<UndoToastHost /></>);
    await screen.findByText("Morning");
    fireEvent.click(screen.getByRole("checkbox", { name: "Select Morning" }));
    const bar = screen.getByTestId("bulk-bar");
    expect(bar).toHaveTextContent("1 schedule selected");
    expect(within(bar).queryByRole("button", { name: "Resume" })).toBeNull();
    fireEvent.click(within(bar).getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(screen.getByText("Paused 1 schedule")).toBeInTheDocument());
  });

  it("select all ticks every schedule and webhook, and a chat row has no checkbox", async () => {
    mockFetch(routes());
    wrap(page(true));
    await screen.findByText("Morning");
    fireEvent.click(screen.getByRole("checkbox", { name: "Select all automations" }));
    expect(screen.getByTestId("bulk-bar")).toHaveTextContent("4 automations selected");
    expect(screen.getAllByRole("checkbox", { name: /^Select /, checked: true })).toHaveLength(5);
    const chatRow = (await screen.findByText(/In chats on/)).closest("li")!;
    expect(within(chatRow).queryByRole("checkbox")).toBeNull();
  });

  it("a mixed selection pauses the ones that are on and deletes by typing the count", async () => {
    const calls: string[] = [];
    mockFetch(
      routes({
        "DELETE /v1/schedules/s1": () => { calls.push("s1"); return json(204, null); },
        "DELETE /v1/webhooks/h1": () => { calls.push("h1"); return json(204, null); },
      }),
    );
    wrap(page(true));
    await screen.findByText("Morning");
    fireEvent.click(screen.getByRole("checkbox", { name: "Select Morning" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select Deploy" }));
    expect(screen.getByTestId("bulk-bar")).toHaveTextContent("2 automations selected");
    fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete 2 automations?" });
    fireEvent.change(within(dialog).getByLabelText(/Type the number of automations to confirm/), { target: { value: "2" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete 2 automations" }));
    await waitFor(() => expect(calls).toEqual(["s1", "h1"]));
  });

  it("only webhooks selected says webhooks", async () => {
    mockFetch(routes());
    wrap(page(true));
    await screen.findByText("Deploy");
    fireEvent.click(screen.getByRole("checkbox", { name: "Select Deploy" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select Alerts" }));
    expect(screen.getByTestId("bulk-bar")).toHaveTextContent("2 webhooks selected");
  });

  it("has no checkbox with Show more controls off, and none for someone who cannot own", async () => {
    mockFetch(routes());
    wrap(page(true), { moreControls: false });
    await screen.findByText("Morning");
    expect(screen.queryByRole("checkbox", { name: /^Select / })).toBeNull();
    cleanup();
    wrap(page(false));
    await screen.findByText("Morning");
    expect(screen.queryByRole("checkbox", { name: /^Select / })).toBeNull();
  });
});
