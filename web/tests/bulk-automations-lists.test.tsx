import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SchedulesTab } from "@/components/schedules/schedules-tab";
import { WebhooksTab } from "@/components/webhooks/webhooks-tab";
import { UndoToastHost } from "@/components/ui/undo-toast";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-19 item 3, Automations: the bar and the checkboxes are on an agent's Schedules and Webhooks lists, for owners,
// with Show more controls on.

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

describe("Schedules list", () => {
  const routes = () => ({ "GET /v1/agents/a1/schedules": () => json(200, { items: [sched("s1", "Morning"), sched("s2", "Evening", false)], next_cursor: null }) });

  it("an owner at Expert can tick schedules and pause them", async () => {
    mockFetch({ ...routes(), "PATCH /v1/schedules/s1": () => json(200, {}) });
    wrap(<><SchedulesTab agentId="a1" canOwn /><UndoToastHost /></>);
    await screen.findByText("Morning");
    fireEvent.click(screen.getByRole("checkbox", { name: "Select Morning" }));
    const bar = screen.getByTestId("bulk-bar");
    expect(bar).toHaveTextContent("1 schedule selected");
    expect(within(bar).queryByRole("button", { name: "Resume" })).toBeNull();
    fireEvent.click(within(bar).getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(screen.getByText("Paused 1 schedule")).toBeInTheDocument());
  });

  it("select all ticks them all", async () => {
    mockFetch(routes());
    wrap(<SchedulesTab agentId="a1" canOwn />);
    await screen.findByText("Morning");
    fireEvent.click(screen.getByRole("checkbox", { name: "Select all schedules" }));
    expect(screen.getByTestId("bulk-bar")).toHaveTextContent("2 schedules selected");
  });

  it("has no checkbox with Show more controls off, and none for someone who cannot own", async () => {
    mockFetch(routes());
    wrap(<SchedulesTab agentId="a1" canOwn />, { moreControls: false });
    await screen.findByText("Morning");
    expect(screen.queryByRole("checkbox")).toBeNull();
    cleanup();
    wrap(<SchedulesTab agentId="a1" canOwn={false} />);
    await screen.findByText("Morning");
    expect(screen.queryByRole("checkbox")).toBeNull();
  });
});

describe("Webhooks list", () => {
  it("an owner at Expert can tick webhooks and delete them by typing the count", async () => {
    const calls: string[] = [];
    mockFetch({
      "GET /v1/agents/a1/webhooks": () => json(200, { items: [hook("h1", "Deploy"), hook("h2", "Alerts")], next_cursor: null }),
      "DELETE /v1/webhooks/h1": () => { calls.push("h1"); return json(204, null); },
      "DELETE /v1/webhooks/h2": () => { calls.push("h2"); return json(204, null); },
    });
    wrap(<WebhooksTab agentId="a1" wsId="w1" canOwn />);
    await screen.findByText("Deploy");
    fireEvent.click(screen.getByRole("checkbox", { name: "Select all webhooks" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete 2 webhooks?" });
    fireEvent.change(within(dialog).getByLabelText(/Type the number of webhooks to confirm/), { target: { value: "2" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete 2 webhooks" }));
    await waitFor(() => expect(calls).toEqual(["h1", "h2"]));
  });

  it("has no checkbox with Show more controls off", async () => {
    mockFetch({ "GET /v1/agents/a1/webhooks": () => json(200, { items: [hook("h1", "Deploy")], next_cursor: null }) });
    wrap(<WebhooksTab agentId="a1" wsId="w1" canOwn />, { moreControls: false });
    await screen.findByText("Deploy");
    expect(screen.queryByRole("checkbox")).toBeNull();
  });
});
