import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { QueuedRow, followupErrorText } from "@/components/chat/status-rows";
import type { Followup } from "@/lib/api/conversations";
import { ApiError } from "@/lib/api/client";

// FIX-225 (D-469, D-470, C-1154): the queued row shows exactly the buttons its `controls` allow, in the order
// Steer | Send now, Edit, Cancel; every click answers on the row; Edit is an inline text box.

type Flags = { steer: boolean; send_now: boolean; edit: boolean; cancel: boolean };
const fup = (controls: Partial<Flags> = {}, over: Partial<Followup> = {}): Followup =>
  ({
    id: "f1",
    parent_turn_id: "t1",
    status: "joining",
    text: "also check CI",
    created_at: "2026-11-08T12:01:00Z",
    speaker_label: "Ann",
    edited_at: null,
    steer_requested: false,
    steer_error: null,
    promoted_turn_id: null,
    controls: { steer: false, send_now: false, edit: false, cancel: false, ...controls },
    ...over,
  }) as Followup;

const ok = () => Promise.resolve({});
function row(f: Followup, over: Partial<Parameters<typeof QueuedRow>[0]> = {}) {
  const fns = { onSteer: vi.fn(ok), onSendNow: vi.fn(ok), onSave: vi.fn(ok), onCancel: vi.fn(ok), ...over };
  const view = render(<QueuedRow followup={f} {...fns} />);
  return { ...view, ...fns } as typeof view & { onSteer: ReturnType<typeof vi.fn>; onSendNow: ReturnType<typeof vi.fn>; onSave: ReturnType<typeof vi.fn>; onCancel: ReturnType<typeof vi.fn> };
}
const names = () => screen.queryAllByRole("button").map((b) => b.textContent);
const apiError = (code: string, message = "api says so", status = 409) => new ApiError(code, message, undefined, status);

describe("which buttons show", () => {
  const LABELS: [keyof Flags, string][] = [
    ["steer", "Steer"],
    ["send_now", "Send now"],
    ["edit", "Edit"],
    ["cancel", "Cancel"],
  ];
  // Every combination of the four flags: a button renders when, and only when, its flag is true.
  for (let mask = 0; mask < 16; mask++) {
    const flags: Flags = { steer: !!(mask & 1), send_now: !!(mask & 2), edit: !!(mask & 4), cancel: !!(mask & 8) };
    const want = LABELS.filter(([k]) => flags[k]).map(([, l]) => l);
    it(`flags ${JSON.stringify(flags)} → ${want.length ? want.join(", ") : "no buttons"}`, () => {
      row(fup(flags));
      expect(names()).toEqual(want);
      expect(screen.getByText("Queued")).toBeTruthy();
      expect(screen.getByText("also check CI")).toBeTruthy();
    });
  }

  it("shows the text on one line (truncated, the whole text in the tooltip) after the Queued badge", () => {
    const long = "x".repeat(300);
    row(fup({ cancel: true }, { text: long }));
    const t = screen.getByText(long);
    expect(t.className).toContain("truncate");
    expect(t.getAttribute("title")).toBe(long);
  });

  it("uses tokens and primitives only: wraps under the text at phone width, no fixed widths", () => {
    const { container } = row(fup({ steer: true, edit: true, cancel: true }));
    const buttons = within(container).getAllByRole("button");
    // At phone width the group is full-width under the text and wraps; from `sm` it sits beside the text.
    const group = buttons[0]!.parentElement!;
    expect(group.className).toMatch(/\bflex-wrap\b/);
    expect(group.className).toMatch(/\bw-full\b/);
    expect(container.innerHTML).not.toMatch(/style=|#[0-9a-f]{3,6}\b|\bw-\[\d/i);
    for (const b of buttons) expect(b.className).toContain("max-md:min-h-11");
  });
});

describe("Steer", () => {
  it("asks, then says it is steering and drops the button", async () => {
    const r = row(fup({ steer: true, edit: true, cancel: true }));
    fireEvent.click(screen.getByRole("button", { name: "Steer" }));
    await waitFor(() => expect(r.onSteer).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("Steering in at the next step…")).toBeTruthy();
    expect(names()).toEqual(["Edit", "Cancel"]);
  });

  it("reads 'Steering in…' from the server's steer_requested, with no Steer button even if the flag lingers", () => {
    row(fup({ steer: true, cancel: true }, { steer_requested: true }));
    expect(screen.getByText("Steering in at the next step…")).toBeTruthy();
    expect(names()).toEqual(["Cancel"]);
  });

  it("disables every button while the request runs", async () => {
    let finish!: () => void;
    const slow = vi.fn(() => new Promise<unknown>((res) => (finish = () => res({}))));
    row(fup({ steer: true, edit: true, cancel: true }), { onSteer: slow });
    fireEvent.click(screen.getByRole("button", { name: "Steer" }));
    await waitFor(() => expect(screen.getAllByRole("button").every((b) => (b as HTMLButtonElement).disabled)).toBe(true));
    await act(async () => finish());
  });

  it("shows the refusal on the row, as an alert, with words for the known codes", async () => {
    const cases: [ApiError, string][] = [
      [apiError("followup_not_waiting"), "This message was already handed to the agent, cancelled or run, so it can't be changed."],
      [apiError("steer_unsupported"), "This agent can't take messages while it works. The message runs when the current turn ends."],
      [apiError("frozen", "This agent is paused.", 423), "This agent is paused."],
      [apiError("brand_new_code", "The API's own words"), "The API's own words"],
    ];
    for (const [err, text] of cases) {
      const { unmount } = row(fup({ steer: true }), { onSteer: vi.fn(() => Promise.reject(err)) });
      fireEvent.click(screen.getByRole("button", { name: "Steer" }));
      const alert = await screen.findByRole("alert");
      expect(alert.textContent).toBe(text);
      // Nothing changed: the button is back and the row does not claim it is steering.
      expect(screen.getByRole("button", { name: "Steer" })).toBeTruthy();
      expect(screen.queryByText("Steering in at the next step…")).toBeNull();
      unmount();
    }
  });
});

describe("Send now", () => {
  it("calls the turn's send-now and writes a refusal on the row", async () => {
    const r = row(fup({ send_now: true }), { onSendNow: vi.fn(() => Promise.reject(apiError("send_now_unsupported"))) });
    expect(names()).toEqual(["Send now"]);
    fireEvent.click(screen.getByRole("button", { name: "Send now" }));
    await waitFor(() => expect(r.onSendNow).toHaveBeenCalledTimes(1));
    expect((await screen.findByRole("alert")).textContent).toBe("This agent can't move a running step to the background.");
  });

  it("shows an outcome that arrived later (nothing was running) on the row", () => {
    row(fup({ send_now: true }), { notice: "Nothing was running to move to the background." });
    expect(screen.getByRole("alert").textContent).toBe("Nothing was running to move to the background.");
  });
});

describe("a steer the harness did not take", () => {
  it("says it will run after the current turn and offers no Steer again", () => {
    row(fup({ steer: true, edit: true, cancel: true }, { steer_requested: true, steer_error: "missed" }));
    expect(screen.getByRole("alert").textContent).toBe("Couldn't steer this in (the agent missed it). It will run after the current turn.");
    expect(screen.queryByText("Steering in at the next step…")).toBeNull();
  });
  it("has its own line when the harness refused it", () => {
    row(fup({ edit: true, cancel: true }, { steer_requested: true, steer_error: "refused" }));
    expect(screen.getByRole("alert").textContent).toMatch(/can't take messages right now\). It will run after the current turn\./);
  });
});

describe("Edit", () => {
  it("turns the row into a text box with Save and Discard changes, and saves the trimmed text", async () => {
    const r = row(fup({ edit: true, cancel: true }));
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const box = screen.getByLabelText("Edit queued message") as HTMLTextAreaElement;
    expect(box.value).toBe("also check CI");
    expect(names()).toEqual(["Save", "Discard changes"]);
    fireEvent.change(box, { target: { value: "  check CI and the docs  " } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(r.onSave).toHaveBeenCalledWith("check CI and the docs"));
    await waitFor(() => expect(screen.queryByLabelText("Edit queued message")).toBeNull());
    expect(names()).toEqual(["Edit", "Cancel"]);
  });

  it("Discard changes closes the box without saving and restores the text next time", () => {
    const r = row(fup({ edit: true }));
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.change(screen.getByLabelText("Edit queued message"), { target: { value: "something else" } });
    fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(r.onSave).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("Edit queued message")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect((screen.getByLabelText("Edit queued message") as HTMLTextAreaElement).value).toBe("also check CI");
  });

  it("Save is off for empty text, and a refusal (too large, no longer waiting) stays inline with the box open", async () => {
    row(fup({ edit: true }), { onSave: vi.fn(() => Promise.reject(apiError("payload_too_large", "Message text is too large.", 413))) });
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const box = screen.getByLabelText("Edit queued message");
    fireEvent.change(box, { target: { value: "   " } });
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(box, { target: { value: "x".repeat(10) } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Message text is too large.");
    expect(screen.getByLabelText("Edit queued message")).toBeTruthy();
  });
});

describe("Cancel", () => {
  it("removes the row", async () => {
    const r = row(fup({ cancel: true }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(r.onCancel).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByText("also check CI")).toBeNull());
  });

  it("keeps the row and says why when it cannot be cancelled", async () => {
    row(fup({ cancel: true }), { onCancel: vi.fn(() => Promise.reject(apiError("followup_not_waiting"))) });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/already handed to the agent/);
    expect(screen.getByText("also check CI")).toBeTruthy();
  });
});

describe("followupErrorText", () => {
  it("falls back to a plain sentence for an error that is not the API's", () => {
    expect(followupErrorText(new Error("boom"))).toBe("Something went wrong. Try again.");
    expect(followupErrorText(apiError("followup_forbidden"))).toBe("Only the person who wrote this message can edit it.");
  });
});
