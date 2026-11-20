import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MemoryTab } from "@/components/memory/memory-tab";
import { baseKey, memoryName, memoryPreview, newMemoryKey, withSuffix } from "@/lib/memory-key";
import { err, json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  usePathname: () => "/o/acme/w/main/agents/daneel",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));

describe("memory keys are made by Aurora (behaviour-1)", () => {
  it("comes from the title, else the first words of the text, in kebab-case ASCII", () => {
    expect(baseKey("Bob's tea", "ignored")).toBe("bobs-tea");
    expect(baseKey("", "Bob prefers green tea in the morning, always.")).toBe("bob-prefers-green-tea-in-the");
    expect(baseKey("Crème brûlée & Zoë", "")).toBe("creme-brulee-zoe");
  });
  it("falls back to note when nothing usable is left", () => {
    expect(baseKey("日本語", "")).toBe("note");
    expect(baseKey("", "   ")).toBe("note");
    expect(baseKey("!!!", "x")).toBe("note");
  });
  it("is at most 64 characters and never ends in a hyphen", () => {
    const k = baseKey("a".repeat(63) + " bbb", "");
    expect(k.length).toBeLessThanOrEqual(64);
    expect(k.endsWith("-")).toBe(false);
    expect(baseKey("word ".repeat(40), "").length).toBeLessThanOrEqual(64);
    const suffixed = withSuffix("a".repeat(64), 12);
    expect(suffixed.length).toBe(64);
    expect(suffixed.endsWith("-12")).toBe(true);
    for (const key of [baseKey("Bob's tea", ""), k, suffixed]) expect(key).toMatch(/^[a-z0-9]+(-[a-z0-9]+)*$/);
  });
  it("is unique against the agent's keys by a numeric suffix", () => {
    expect(newMemoryKey("Bob's tea", "", ["other"])).toBe("bobs-tea");
    expect(newMemoryKey("Bob's tea", "", ["bobs-tea"])).toBe("bobs-tea-2");
    expect(newMemoryKey("Bob's tea", "", ["bobs-tea", "bobs-tea-2"])).toBe("bobs-tea-3");
    // A retry after a refusal moves on to the next free one.
    expect(newMemoryKey("Bob's tea", "", [], 1)).toBe("bobs-tea-2");
  });
  it("names a memory by its title, else the first line cut at 60 characters, and previews its text", () => {
    expect(memoryName({ title: "Bob's tea", content: "x" })).toBe("Bob's tea");
    expect(memoryName({ content: "\n# Heading line\nmore" })).toBe("Heading line");
    expect(memoryName({ content: "w".repeat(80) }).length).toBe(60);
    expect(memoryPreview({ title: "Bob's tea", content: "Bob prefers green tea.\nmore" })).toBe("Bob prefers green tea.");
    expect(memoryPreview({ content: "First line\nSecond line" })).toBe("Second line");
    expect(memoryPreview({ title: "T", description: "A description", content: "x" })).toBe("A description");
  });
});

const mem = (over: Record<string, unknown> = {}) => ({
  key: "bobs-tea", title: "Bob's tea", description: null, content: "Bob prefers green tea in the morning.",
  index_line: "bobs-tea: Bob's tea", revision: 1, created_at: "2026-11-20T10:00:00Z", updated_at: "2026-11-20T10:00:00Z", ...over,
});

describe("the memory page", () => {
  it("lists memories by title with a preview, never the key", async () => {
    mockFetch({ "GET /v1/agents/a1/memories": () => json(200, { items: [mem()], next_cursor: null }) });
    wrap(<MemoryTab agentId="a1" agentName="Dani" />);
    const list = await screen.findByRole("list", { name: "Memories" });
    expect(within(list).getByText("Bob's tea")).toBeInTheDocument();
    expect(within(list).getByText("Bob prefers green tea in the morning.")).toBeInTheDocument();
    expect(list.textContent).not.toContain("bobs-tea");
  });

  it("writes a memory in one box: no Key, Description or Markdown label; the key is made and sent", async () => {
    const posts: Record<string, unknown>[] = [];
    mockFetch({
      "GET /v1/agents/a1/memories": () => json(200, { items: [mem()], next_cursor: null }),
      "POST /v1/agents/a1/memories": async (r) => {
        const body = (await r.json()) as Record<string, unknown>;
        posts.push(body);
        return json(201, mem({ key: body.key, title: body.title, content: body.content }));
      },
    });
    wrap(<MemoryTab agentId="a1" agentName="Dani" />);
    fireEvent.click(await screen.findByRole("button", { name: "New memory" }));
    expect(screen.getByLabelText("What should Dani remember?")).toBeInTheDocument();
    expect(screen.getByText("Plain words are fine. Dani reads this before it answers.")).toBeInTheDocument();
    expect(screen.getByLabelText("Title (optional)")).toBeInTheDocument();
    for (const gone of ["Key", "Description", "Content (Markdown)"]) expect(screen.queryByLabelText(gone)).toBeNull();
    fireEvent.change(screen.getByLabelText("What should Dani remember?"), { target: { value: "Bob also likes oat milk." } });
    fireEvent.change(screen.getByLabelText("Title (optional)"), { target: { value: "Bob's tea" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(posts).toHaveLength(1));
    // "bobs-tea" is taken, so the made key carries the next suffix.
    expect(posts[0]).toMatchObject({ key: "bobs-tea-2", title: "Bob's tea", content: "Bob also likes oat milk." });
  });

  it("retries once with the next suffix when the API says the key exists", async () => {
    const keys: string[] = [];
    mockFetch({
      "GET /v1/agents/a1/memories": () => json(200, { items: [], next_cursor: null }),
      "POST /v1/agents/a1/memories": async (r) => {
        const body = (await r.json()) as { key: string };
        keys.push(body.key);
        return keys.length === 1 ? err(409, "conflict", "A memory with this key already exists.") : json(201, mem({ key: body.key }));
      },
    });
    wrap(<MemoryTab agentId="a1" agentName="Dani" />);
    fireEvent.click(await screen.findByRole("button", { name: "New memory" }));
    fireEvent.change(screen.getByLabelText("What should Dani remember?"), { target: { value: "Tea" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(keys).toEqual(["tea", "tea-2"]));
  });

  it("refuses text over the limit with a sentence, and empty text with one", async () => {
    mockFetch({ "GET /v1/agents/a1/memories": () => json(200, { items: [mem()], next_cursor: null }) });
    wrap(<MemoryTab agentId="a1" agentName="Dani" />);
    fireEvent.click(await screen.findByRole("button", { name: "New memory" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Write what Dani should remember.")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("What should Dani remember?"), { target: { value: "x".repeat(100_001) } });
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("opens a memory in the same one box, with Cancel, history and the key only in Technical details", async () => {
    mockFetch({ "GET /v1/agents/a1/memories": () => json(200, { items: [mem()], next_cursor: null }) });
    wrap(<MemoryTab agentId="a1" agentName="Dani" />, { moreControls: true });
    fireEvent.click(await screen.findByText("Bob's tea"));
    expect((screen.getByLabelText("What should Dani remember?") as HTMLTextAreaElement).value).toBe("Bob prefers green tea in the morning.");
    expect((screen.getByLabelText("Title (optional)") as HTMLInputElement).value).toBe("Bob's tea");
    expect(screen.getByRole("button", { name: "History" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
    expect(screen.getByText("Memory key: bobs-tea")).toBeInTheDocument();
  });
});
