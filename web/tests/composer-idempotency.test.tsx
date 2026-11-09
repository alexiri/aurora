import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Composer } from "@/components/chat/composer";
import type { TranscriptState } from "@/lib/chat/transcript";

// P3-018 (RFE-018): the composer sends one `Idempotency-Key` per message; a retry of the same message
// reuses it (so a lost response cannot duplicate the turn) and a different message never does.

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const state = { events: [], deltas: {}, activity: {}, purged: {}, lastSeq: 0 } as unknown as TranscriptState;

interface Sent { key: string | null; body: { text: string } }
function mock(answers: Array<() => Response>) {
  const sent: Sent[] = [];
  let n = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      if (req.method === "POST" && new URL(req.url, "http://localhost").pathname === "/v1/conversations/c1/messages") {
        sent.push({ key: req.headers.get("idempotency-key"), body: JSON.parse(await req.clone().text()) });
        return answers[Math.min(n++, answers.length - 1)]();
      }
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return sent;
}

function mount() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <Composer agentId="a1" convId="c1" base="/o/x/w/y" agentName="Daneel" activeTurn={null} state={state} />
    </QueryClientProvider>,
  );
  return screen.getByLabelText("Message") as HTMLTextAreaElement;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("composer Idempotency-Key", () => {
  it("a retried submit reuses the key; the next message gets a new one", async () => {
    const sent = mock([
      () => json(503, { error: { code: "unavailable", message: "down" } }),
      () => json(202, { turn: { id: "t1" }, followup: null }),
      () => json(202, { turn: { id: "t2" }, followup: null }),
    ]);
    const box = mount();
    fireEvent.change(box, { target: { value: "hello" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(sent).toHaveLength(1));
    // The send failed: the text stays and the same message is retried.
    await waitFor(() => expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(false));
    expect(box.value).toBe("hello");
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(sent).toHaveLength(2));
    await waitFor(() => expect(box.value).toBe(""));
    expect(sent[0].key).toBeTruthy();
    expect(sent[1].key).toBe(sent[0].key);
    expect(sent[1].body).toEqual(sent[0].body);

    fireEvent.change(box, { target: { value: "hello" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(sent).toHaveLength(3));
    expect(sent[2].key).toBeTruthy();
    expect(sent[2].key).not.toBe(sent[0].key);
  });

  it("an edited message after a failure never reuses the key", async () => {
    const sent = mock([
      () => json(503, { error: { code: "unavailable", message: "down" } }),
      () => json(202, { turn: { id: "t1" }, followup: null }),
    ]);
    const box = mount();
    fireEvent.change(box, { target: { value: "first draft" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(sent).toHaveLength(1));
    await waitFor(() => expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.change(box, { target: { value: "second draft" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[1].key).toBeTruthy();
    expect(sent[1].key).not.toBe(sent[0].key);
  });
});
