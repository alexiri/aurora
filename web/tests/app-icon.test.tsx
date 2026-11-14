import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render as rtlRender, screen } from "@testing-library/react";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AttachmentCard } from "@/components/chat/attachments";
import { TranscriptView } from "@/components/chat/transcript-view";
import { AppIcon, appIconUrl } from "@/components/ui/app-icon";
import { emptyTranscript, loadHistory, type TranscriptState } from "@/lib/chat/transcript";

// A person's attachment has no filename in the transcript; the card asks the server (FIX-099), so cards render
// inside a query client. Nothing here is served: a name lookup just fails and the card keeps "Attachment".
const render = (ui: ReactElement) =>
  rtlRender(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);
beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 404 })));
});

afterEach(cleanup);

const imgs = (c: HTMLElement) => Array.from(c.querySelectorAll("img"));

describe("AppIcon", () => {
  it("renders the app's icon route, and the dark variant too when the app has one", () => {
    const { container } = render(<AppIcon appId="github" name="GitHub" hasDarkIcon />);
    const [light, dark] = imgs(container);
    expect(light.getAttribute("src")).toBe("/v1/apps/github/icon");
    expect(light.className).toContain("dark:hidden");
    expect(dark.getAttribute("src")).toBe("/v1/apps/github/icon?variant=dark");
    expect(dark.className).toContain("dark:block");
    expect(dark.className).toContain("hidden");
  });

  it("shows vendor artwork unaltered: no filter, tint, rounding or overlay on the image", () => {
    const { container } = render(<AppIcon appId="aws-keys" name="AWS" hasDarkIcon />);
    for (const img of imgs(container)) {
      expect(img.className).toContain("object-contain");
      expect(img.className).not.toMatch(/rounded|filter|invert|grayscale|opacity|brightness|blur|mix-blend|overflow/);
      expect(img.getAttribute("style")).toBeNull();
    }
    expect(container.querySelectorAll("img + *:not(img)")).toHaveLength(0);
  });

  it("renders one image when there is no dark icon, and escapes the id", () => {
    const { container } = render(<AppIcon appId="a b/c" name="Notion" />);
    expect(imgs(container)).toHaveLength(1);
    expect(imgs(container)[0].getAttribute("src")).toBe(appIconUrl("a b/c"));
    expect(imgs(container)[0].getAttribute("src")).toBe("/v1/apps/a%20b%2Fc/icon");
  });

  it("falls back to a monogram tile when the image fails", () => {
    const { container } = render(<AppIcon appId="linear" name="Linear" />);
    fireEvent.error(imgs(container)[0]);
    expect(imgs(container)).toHaveLength(0);
    const tile = screen.getByTestId("app-icon-fallback");
    expect(tile.textContent).toBe("L");
    expect(tile.className).toContain("bg-accent-soft");
    expect(tile.getAttribute("aria-hidden")).toBe("true");
  });

  it("shows the monogram without a request when the app has no icon", () => {
    const { container } = render(<AppIcon appId="x" name="  sentry" hasIcon={false} />);
    expect(imgs(container)).toHaveLength(0);
    expect(screen.getByTestId("app-icon-fallback").textContent).toBe("s");
  });

  it("a failed dark icon falls back to the light icon, not to the monogram", () => {
    const { container } = render(<AppIcon appId="github" name="GitHub" hasDarkIcon />);
    fireEvent.error(imgs(container)[1]);
    const left = imgs(container);
    expect(left).toHaveLength(1);
    expect(left[0].className).not.toContain("dark:hidden");
    expect(screen.queryByTestId("app-icon-fallback")).toBeNull();
  });
});

describe("expired attachments (FU-P2-015a, C-196)", () => {
  it("renders a chip, never a link, download or thumbnail", () => {
    const { container } = render(<AttachmentCard id="a1" filename="report.png" contentType="image/png" size={10} purged="retention" />);
    expect(screen.getByText("Expired by retention")).toBeTruthy();
    expect(screen.getByText("report.png")).toBeTruthy();
    expect(container.querySelector("a, img, button")).toBeNull();
  });

  it("says removed for a file a person deleted", () => {
    render(<AttachmentCard id="a1" filename="x.txt" purged="deleted" />);
    expect(screen.getByText("Removed")).toBeTruthy();
  });

  it("a live attachment keeps its download link", () => {
    const { container } = render(<AttachmentCard id="a1" filename="x.txt" />);
    expect(container.querySelector("a")?.getAttribute("href")).toBe("/v1/attachments/a1/content");
  });

  it("the transcript marks attachment events and queued turns from the page's purged list", async () => {
    const s: TranscriptState = await loadHistory(async () => ({
      items: [
        { seq: 1, type: "turn.queued", ts: "t", payload: { turn_id: "t1", text: "hi", attachments: ["a-old", "a-new"] } },
        { seq: 2, type: "attachment", ts: "t", payload: { turn_id: "t1", attachment_id: "a-old", filename: "old.pdf", size: 3 } },
      ],
      last_seq: 2,
      purged_attachments: [{ id: "a-old", reason: "retention" }],
    }), emptyTranscript);
    expect(s.purged).toEqual({ "a-old": "retention" });
    const { container } = render(<TranscriptView state={s} wsId="w" base="/b" convId="c" />);
    expect(screen.getAllByTestId("attachment-expired")).toHaveLength(2);
    const links = Array.from(container.querySelectorAll("a")).map((a) => a.getAttribute("href"));
    expect(links).toEqual(["/v1/attachments/a-new/content"]);
  });
});
