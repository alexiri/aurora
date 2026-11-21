import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";

import { AttachmentCard } from "@/components/chat/attachments";
import { effectiveContentType } from "@/lib/api/attachments";
import { buildRows, conversationTitle, searchTime } from "@/lib/chat/layout";
import { errorCopy, type TranscriptState } from "@/lib/chat/transcript";
import { setDisplayTimeZone } from "@/lib/format-date";
import { SOMETHING_WENT_WRONG } from "@/lib/labels";

// P5-REFINE-11: chat-7 (one error), chat-9 (search time), chat-10 (image thumbnail).

const ev = (seq: number, type: string, payload: Record<string, unknown>) => ({ seq, type, ts: "2026-11-08T12:00:00Z", payload });
const stateOf = (events: unknown[]) => ({ events, deltas: {}, activity: {}, purged: {}, lastSeq: events.length }) as unknown as TranscriptState;

describe("chat-7: an error shows once", () => {
  it("does not render the agent's own bubble that repeats the error message", () => {
    const rows = buildRows(
      stateOf([
        ev(1, "turn.queued", { turn_id: "t1", speaker_label: "Bob", text: "Hi" }),
        ev(2, "text", { turn_id: "t1", message_id: "m1", text: "API Error: 400 No connected db." }),
        ev(3, "error", { turn_id: "t1", code: "weird", message: "API Error: 400 No connected db." }),
      ]),
      new Date("2026-11-08T13:00:00Z"),
    );
    const blocks = rows.flatMap((r) => (r.kind === "group" ? r.blocks.map((b) => b.kind) : []));
    expect(blocks).toEqual(["bubble", "error"]);
  });
  it("keeps an unrelated agent text of the same turn", () => {
    const rows = buildRows(
      stateOf([ev(1, "text", { turn_id: "t1", text: "Partial answer" }), ev(2, "error", { turn_id: "t1", code: "weird", message: "API Error: 400" })]),
      new Date("2026-11-08T13:00:00Z"),
    );
    expect(rows.flatMap((r) => (r.kind === "group" ? r.blocks.map((b) => b.kind) : []))).toEqual(["bubble", "error"]);
  });
  it("an unknown code reads the one plain sentence and keeps the technical line for Details", () => {
    const c = errorCopy("weird_new_code", "API Error: 400 No connected db.");
    expect(c.message).toBe(SOMETHING_WENT_WRONG);
    expect(c.message).not.toContain("API Error");
    expect(c.showServerMessage).toBe(true);
  });
});

describe("chat-9: a search result's time", () => {
  it("is the date and the clock", () => {
    setDisplayTimeZone("UTC");
    expect(searchTime("2026-11-19T23:50:00Z", new Date("2026-11-20T10:00:00Z"))).toBe("19 Nov 23:50");
    expect(searchTime("2025-03-02T08:05:00Z", new Date("2026-11-20T10:00:00Z"))).toBe("2 Mar 2025 08:05");
    expect(searchTime(null, new Date())).toBe("");
    // (b2#18) A run thread's title is in the person's zone, like every other time on the screen.
    setDisplayTimeZone("Asia/Tokyo");
    try {
      const run = { title: "x", kind: "schedule", external_ref: "r", automation: { id: "s", kind: "schedule", name: "Morning", deleted: false }, run_at: "2026-11-19T23:50:00Z" };
      expect(conversationTitle(run as never, new Date("2026-11-20T10:00:00Z"))).toBe("Morning · 20 Nov 08:50");
    } finally {
      setDisplayTimeZone("UTC");
    }
  });
});

describe("chat-10: an image is an inline thumbnail with its name and size", () => {
  it("decides by the declared type, else by the name's extension", () => {
    expect(effectiveContentType("image/png", "a.bin")).toBe("image/png");
    expect(effectiveContentType("application/octet-stream", "pic.PNG")).toBe("image/png");
    expect(effectiveContentType(null, "photo.jpeg")).toBe("image/jpeg");
    expect(effectiveContentType("application/octet-stream", "notes.txt")).toBe("application/octet-stream");
  });
  const card = (props: Record<string, unknown>) =>
    render(
      <QueryClientProvider client={new QueryClient()}>
        <AttachmentCard {...({ id: "a1", ...props } as { id: string })} />
      </QueryClientProvider>,
    );
  it("pic.png (octet-stream from an upload) is a thumbnail and reads 179 B · Image", () => {
    const { container } = card({ filename: "pic.png", contentType: "application/octet-stream", size: 179 });
    expect(container.querySelector("img")).not.toBeNull();
    expect(screen.getByText("179 B · Image")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "pic.png" })).toBeInTheDocument();
  });
  it("notes.txt keeps the file card and reads 44 B · File", () => {
    const { container } = card({ filename: "notes.txt", contentType: "text/plain", size: 44 });
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByText("44 B · File")).toBeInTheDocument();
  });
});
