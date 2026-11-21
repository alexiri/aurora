import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { InstructionsTab } from "@/components/agents/instructions-tab";
import { ConversationRow } from "@/components/chat/conversation-list";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { useSaveBar } from "@/components/ui/save-bar";
import type { Agent } from "@/lib/api/queries";
import { markStopped, resetStopped } from "@/lib/chat/stopped";
import { json } from "./support/api";

// P5-REFINE-20 send-back, batch 2 (lane A): behaviour-6 (the Saved flash survives the remount a save causes), channels-2
// (a dialog's actions are its pinned footer), chat-4 (the row's Working goes with Stop).

afterEach(() => {
  resetStopped();
  vi.useRealTimers();
});

const agent = (over: Partial<Agent> = {}): Agent =>
  ({
    id: "a1", org_id: "o1", workspace_id: "w1", handle: "bot", name: "Bot", kind: "byo",
    is_default: false, harness: null, model_provider: null, model_endpoint_id: null, model_id: null,
    effort: null, proactive_messaging: "ask", brief: "Be brief", avatar_url: null, subagent_model: "inherit", model_roles: null,
    created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
    ...over,
  }) as Agent;

/** What `AgentPage` does: keep the flash above the form and re-key the form with `updated_at`. */
function Page({ initial }: { initial: Agent }) {
  const bar = useSaveBar();
  const [sig, setSig] = useState<string | null>(null);
  const [current, setCurrent] = useState(initial);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      if (req?.method === "PATCH") {
        const body = (await req.clone().json()) as Partial<Agent>;
        const next = { ...current, ...body, updated_at: "2026-01-02T00:00:00Z" } as Agent;
        // The refetch that follows a save arrives with a new `updated_at`: the page re-keys the form.
        setTimeout(() => setCurrent(next), 0);
        return json(200, next);
      }
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return <InstructionsTab key={current.updated_at} agent={current} wsId="w1" saveState={{ bar, sig, setSig }} />;
}

describe("Instructions: Saved (behaviour-6)", () => {
  it("shows Saved after Save although the form is remounted with the refreshed agent, then lets it go after 2 s", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <Page initial={agent()} />
      </QueryClientProvider>,
    );
    fireEvent.change(await screen.findByLabelText("Instructions"), { target: { value: "Be very brief" } });
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    // The bar says Saved even after the remount (the textarea now holds the saved text from the new agent).
    expect(await screen.findByTestId("save-bar-saved")).toHaveTextContent("Saved");
    await waitFor(() => expect((screen.getByLabelText("Instructions") as HTMLTextAreaElement).value).toBe("Be very brief"));
    expect(screen.getByTestId("save-bar-saved")).toBeInTheDocument();
    expect(screen.queryByText("Unsaved changes")).toBeNull();
    await waitFor(() => expect(screen.queryByTestId("save-bar-saved")).toBeNull(), { timeout: 3000 });
  });
});

describe("FormDialog (channels-2)", () => {
  it("keeps Cancel and the submit button in the pinned footer, outside the scrolling form, and still submits", () => {
    const onSubmit = vi.fn((e: { preventDefault: () => void }) => e.preventDefault());
    render(
      <FormDialog open onOpenChange={() => {}} title="New thing" submitLabel="Create thing" onSubmit={onSubmit as never}>
        <input aria-label="Name" />
      </FormDialog>,
    );
    const form = screen.getByLabelText("Name").closest("form")!;
    const create = screen.getByRole("button", { name: "Create thing" });
    expect(form.contains(create)).toBe(false);
    expect(create.getAttribute("form")).toBe(form.id);
    expect(form.closest("div.overflow-y-auto")).not.toBeNull(); // the body scrolls
    expect(create.closest("div.overflow-y-auto")).toBeNull(); // the footer does not
    fireEvent.click(create);
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });
});

describe("Conversation row (chat-4)", () => {
  it("drops Working as soon as Stop was pressed for its turn", () => {
    const c = {
      id: "c1", agent_id: "a1", kind: "direct", external_ref: "c1", title: "Summarise the quarterly planning notes", title_source: "auto",
      direct_user_id: null, direct_user: null, origin_conversation_id: null, last_seq: 1, app_turn_streak: 0, app_turn_capped: false,
      active_turn: { id: "t1", abort_requested: false }, last_activity_at: new Date().toISOString(), archived_at: null, automation: null,
      run_at: null, created_at: "2026-01-01T00:00:00Z",
    };
    const qc = new QueryClient();
    render(
      <QueryClientProvider client={qc}>
        <ConversationRow conversation={c as never} selected={false} now={new Date()} onSelect={() => {}} />
      </QueryClientProvider>,
    );
    expect(screen.getByText("Working")).toBeInTheDocument();
    act(() => markStopped("t1"));
    expect(screen.queryByText("Working")).toBeNull();
  });
});
