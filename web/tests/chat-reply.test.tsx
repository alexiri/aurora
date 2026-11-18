import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import { ChatActionsContext, excerptOf, type ReplyTarget } from "@/components/chat/chat-actions";
import { Composer } from "@/components/chat/composer";
import { JumpToLatest } from "@/components/chat/jump-to-latest";
import { MessageGroup } from "@/components/chat/message-group";
import { QuoteStrip, quoteOf } from "@/components/chat/quote-strip";
import { ReplyBar } from "@/components/chat/reply-bar";
import { buildPermalink, permalinkSeq, withoutMessageParam } from "@/lib/chat/permalink";
import type { Row } from "@/lib/chat/layout";
import type { TranscriptState } from "@/lib/chat/transcript";
import { json } from "./support/api";

// P5-CHAT-2 (D-228, C-725): reply bar, quote strip, jump to latest, permalinks, message actions.

const state = { events: [], deltas: {}, activity: {}, purged: {}, lastSeq: 0 } as unknown as TranscriptState;
const target: ReplyTarget = { seq: 7, author: "Daneel", excerpt: "Shall we ship it?" };

describe("permalinks", () => {
  it("builds ?c=&m= on the current page and drops other parameters", () => {
    expect(buildPermalink("https://x.io/o/a/w/b/agents/daneel?tab=chat&draft=hi#x", "c1", 12)).toBe(
      "https://x.io/o/a/w/b/agents/daneel?c=c1&m=12",
    );
  });
  it("reads m only for this conversation and only as a positive integer", () => {
    expect(permalinkSeq("?c=c1&m=12", "c1")).toBe(12);
    expect(permalinkSeq("?m=12", "c1")).toBe(12);
    expect(permalinkSeq("?c=other&m=12", "c1")).toBeNull();
    for (const bad of ["?m=0", "?m=-1", "?m=1.5", "?m=abc", "?m=", ""]) expect(permalinkSeq(bad, "c1")).toBeNull();
  });
  it("drops m after the jump", () => {
    expect(withoutMessageParam("https://x.io/p?c=c1&m=3&tab=chat")).toBe("/p?c=c1&tab=chat");
  });
});

describe("excerpt", () => {
  it("clamps on a word boundary with an ellipsis", () => {
    const e = excerptOf("word ".repeat(100));
    expect(e.length).toBeLessThanOrEqual(280);
    expect(e.endsWith("word…")).toBe(true);
    expect(excerptOf("short")).toBe("short");
  });
});

describe("reply bar", () => {
  it("shows the author and excerpt and cancels", () => {
    const cancel = vi.fn();
    render(<ReplyBar target={target} onCancel={cancel} />);
    expect(screen.getByText("Replying to Daneel")).toBeTruthy();
    expect(screen.getByText("Shall we ship it?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel reply" }));
    expect(cancel).toHaveBeenCalled();
  });
  it("shows an error line", () => {
    render(<ReplyBar target={target} onCancel={() => undefined} error="You can't reply to that message any more" />);
    expect(screen.getByRole("alert").textContent).toContain("any more");
  });
});

describe("quote strip", () => {
  it("jumps on click, with author and a clamped plain excerpt", () => {
    const jump = vi.fn();
    render(<QuoteStrip quote={{ seq: 7, author_label: "Daneel", excerpt: "see [docs](https://x.io)" }} onJump={jump} side="person" />);
    const strip = screen.getByTestId("quote-strip");
    expect(strip.textContent).toContain("Daneel");
    expect(strip.querySelector("a")).toBeNull();
    fireEvent.click(strip);
    expect(jump).toHaveBeenCalledWith(7);
  });
  it("a redacted original is a neutral line, not a button", () => {
    render(<QuoteStrip quote={{ seq: 7, redacted: true }} onJump={() => undefined} side="agent" />);
    expect(screen.getByText("Original message redacted")).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
  });
  it("reads payload.reply_to defensively", () => {
    expect(quoteOf({ reply_to: { seq: 3, author_label: "A", excerpt: "x" } })).toMatchObject({ seq: 3, author_label: "A" });
    expect(quoteOf({ reply_to: { seq: 3, redacted: true } })).toMatchObject({ redacted: true });
    expect(quoteOf({ reply_to: "nope" })).toBeNull();
    expect(quoteOf({})).toBeNull();
  });
});

describe("jump to latest", () => {
  it("counts new messages and scrolls on click", () => {
    const click = vi.fn();
    const { rerender } = render(<JumpToLatest count={0} onClick={click} />);
    expect(screen.getByRole("button").textContent).toBe("Jump to latest");
    rerender(<JumpToLatest count={3} onClick={click} />);
    expect(screen.getByRole("button").textContent).toContain("· 3 new");
    fireEvent.click(screen.getByRole("button"));
    expect(click).toHaveBeenCalled();
  });
});

function group(): Row & { kind: "group" } {
  const ev = (seq: number, type: string, payload: Record<string, unknown>) => ({ seq, type, payload, ts: "2026-11-14T10:00:00Z" });
  return {
    kind: "group",
    key: "g1",
    side: "agent",
    name: "Daneel",
    ts: "2026-11-14T10:00:00Z",
    door: null,
    blocks: [
      { kind: "bubble", ev: ev(7, "text", { text: "Shall we ship it?" }) },
      { kind: "tools", key: "t1", calls: [], running: false },
      { kind: "approval", ev: ev(8, "approval.pending", { summary: "x" }) },
      { kind: "attachment", ev: ev(9, "attachment", { attachment_id: "a1", filename: "report.pdf" }) },
    ],
  } as unknown as Row & { kind: "group" };
}

describe("message actions", () => {
  it("Reply on a message and on an agent file; none on tool groups or approvals; R replies", () => {
    vi.stubGlobal("fetch", vi.fn(async () => json(404, {})));
    const reply = vi.fn();
    const qc = new QueryClient();
    render(
      <QueryClientProvider client={qc}>
        <ChatActionsContext.Provider value={{ reply, jump: () => undefined, link: (s) => `https://x.io/?m=${s}` }}>
          <MessageGroup row={group()} state={state} wsId="w" base="/o/a/w/b" convId="c1" agent={{ id: "ag", name: "Daneel" } as never} />
        </ChatActionsContext.Provider>
      </QueryClientProvider>,
    );
    const buttons = screen.getAllByRole("button", { name: "Reply" });
    expect(buttons).toHaveLength(2);
    fireEvent.click(buttons[0]);
    expect(reply).toHaveBeenCalledWith({ seq: 7, author: "Daneel", excerpt: "Shall we ship it?" });
    fireEvent.click(buttons[1]);
    expect(reply).toHaveBeenLastCalledWith(expect.objectContaining({ seq: 9, excerpt: "report.pdf" }));
    reply.mockClear();
    fireEvent.keyDown(buttons[0], { key: "r" });
    expect(reply).toHaveBeenCalledWith(expect.objectContaining({ seq: 7 }));
  });
});

describe("composer reply", () => {
  function mount(answers: Array<() => Response>) {
    const sent: Array<{ text: string; reply_to?: { seq: number } }> = [];
    let n = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
        if (req.method === "POST" && new URL(req.url, "http://localhost").pathname === "/v1/conversations/c1/messages") {
          sent.push(JSON.parse(await req.clone().text()));
          return answers[Math.min(n++, answers.length - 1)]();
        }
        return json(404, { error: { code: "not_found", message: "nf" } });
      }),
    );
    function Host() {
      const [reply, setReply] = useState<ReplyTarget | null>(target);
      return (
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <Composer
            agentId="a1"
            convId="c1"
            base="/o/x/w/y"
            agentName="Daneel"
            activeTurn={null}
            state={state}
            reply={reply}
            onClearReply={() => setReply(null)}
          />
        </QueryClientProvider>
      );
    }
    render(<Host />);
    return { sent, box: screen.getByLabelText("Message") as HTMLTextAreaElement };
  }

  it("sends reply_to.seq, then clears the bar", async () => {
    const { sent, box } = mount([() => json(202, { turn: { id: "t1" }, followup: null })]);
    expect(screen.getByText("Replying to Daneel")).toBeTruthy();
    fireEvent.change(box, { target: { value: "yes" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toMatchObject({ text: "yes", reply_to: { seq: 7 } });
    await waitFor(() => expect(screen.queryByText("Replying to Daneel")).toBeNull());
  });

  it("Esc cancels the reply and keeps the text", () => {
    const { box } = mount([() => json(202, {})]);
    fireEvent.change(box, { target: { value: "draft" } });
    fireEvent.keyDown(box, { key: "Escape" });
    expect(screen.queryByText("Replying to Daneel")).toBeNull();
    expect(box.value).toBe("draft");
  });

  it("a 422 reply_target_invalid says so in the bar and keeps the text", async () => {
    const { box } = mount([() => json(422, { error: { code: "reply_target_invalid", message: "no", details: { reason: "redacted" } } })]);
    fireEvent.change(box, { target: { value: "keep me" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("You can't reply to that message any more"));
    expect(box.value).toBe("keep me");
  });
});
