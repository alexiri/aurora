import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AgentState, agentStateOf, CANT_ANSWER_REASON } from "@/components/agents/agent-state";
import { json } from "./support/api";

// P5-REFINE-11 item 2 (D-408): one header state, most urgent first: Can't answer → Needs you → Working → Starting its
// computer… → Asleep → Ready; Ready only when the computer answers.

const on = { word: "on", ready: true, working: false, asleep: false };
const base = { cantAnswer: false, needs: 0, working: false };

describe("agentStateOf", () => {
  it("orders Can't answer → Needs you → Working → Starting → Asleep → Ready", () => {
    const starting = { word: "starting", ready: false, working: false, asleep: false };
    const asleep = { word: "off", ready: false, working: false, asleep: true };
    expect(agentStateOf({ ...base, cantAnswer: true, needs: 2, working: true, computer: starting }).word).toBe("Can't answer");
    expect(agentStateOf({ ...base, needs: 2, working: true, computer: starting }).word).toBe("Needs you");
    expect(agentStateOf({ ...base, working: true, computer: starting }).word).toBe("Working");
    expect(agentStateOf({ ...base, computer: starting })).toEqual({ word: "Starting its computer…", tone: "idle" });
    expect(agentStateOf({ ...base, computer: asleep })).toEqual({ word: "Asleep", tone: "idle" });
    expect(agentStateOf({ ...base, computer: on })).toEqual({ word: "Ready", tone: "ok" });
  });
  it("is Working while the computer reports a turn or a process", () => {
    expect(agentStateOf({ ...base, computer: { ...on, working: true } }).word).toBe("Working");
  });
  it("is Ready without computer data (not loaded, BYO): today's words, no computer states", () => {
    expect(agentStateOf(base).word).toBe("Ready");
    expect(agentStateOf({ ...base, computer: null }).word).toBe("Ready");
  });
  it("a computer that failed to start cannot answer", () => {
    expect(agentStateOf({ ...base, computer: { word: "failed", ready: false, working: false, asleep: false } }).word).toBe("Can't answer");
  });
});

afterEach(() => vi.unstubAllGlobals());

function renderState(readiness: Record<string, unknown>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const url = new URL(input instanceof Request ? input.url : String(input), "http://localhost");
      if (url.pathname === "/v1/agents/a1/answer-readiness") return json(200, readiness);
      if (url.pathname === "/v1/agents/a1/sandbox") return json(200, { computer: on });
      return json(200, { items: [], next_cursor: null });
    }),
  );
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <AgentState agentId="a1" wsId="w1" canApprove={false} hosted action={<button type="button">Fix it</button>} />
    </QueryClientProvider>,
  );
}

describe("AgentState (header)", () => {
  it.each([
    ["no_ai_key", "Can't answer: no AI key"],
    ["key_not_usable", "Can't answer: the key stopped working"],
    ["no_model", "Can't answer: choose a model"],
  ] as const)("says why it cannot answer (%s) and shows the action slot beside it", async (reason, words) => {
    renderState({ can_answer: false, reason, usable_keys: 0, fix: null });
    await waitFor(() => expect(screen.getByTestId("agent-state")).toHaveTextContent(words));
    expect(CANT_ANSWER_REASON[reason]).toBe(words.replace("Can't answer: ", ""));
    expect(screen.getByRole("button", { name: "Fix it" })).toBeInTheDocument();
  });
  it("says Ready when it can answer and the computer answers, with no action", async () => {
    renderState({ can_answer: true, reason: null, usable_keys: 1, fix: null });
    await waitFor(() => expect(screen.getByTestId("agent-state")).toHaveTextContent("Ready"));
    expect(screen.queryByRole("button", { name: "Fix it" })).toBeNull();
  });
});
