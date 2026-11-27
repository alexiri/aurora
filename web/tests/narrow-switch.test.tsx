import { act, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { MemoryTab } from "@/components/memory/memory-tab";
import { NARROW_SCREEN } from "@/components/ui/list-detail";
import { json, type Handler } from "./support/api";
import { wrap } from "./support/render";
import { AgentSkills, applied, effective, me, openMenu, routes, skill } from "./support/skills";

// A window dragged across the list/detail breakpoint (or a phone turned sideways) while a form is open: what was typed,
// and what was opened, is still there in the other layout (`ui/list-detail`, the Memory and Skills pages).

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  usePathname: () => "/o/acme/w/main/agents/daneel",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

/** A `matchMedia` whose answer to the narrow query can change, telling its listeners as a browser does. */
function screenWidth(startNarrow: boolean) {
  let narrow = startNarrow;
  const listeners = new Set<() => void>();
  vi.stubGlobal("matchMedia", (query: string) => ({
    get matches() {
      return query === NARROW_SCREEN && narrow;
    },
    media: query,
    addEventListener: (_: string, f: () => void) => listeners.add(f),
    removeEventListener: (_: string, f: () => void) => listeners.delete(f),
  }));
  vi.stubGlobal("scrollTo", () => undefined);
  return (next: boolean) =>
    act(() => {
      narrow = next;
      for (const f of [...listeners]) f();
    });
}

const type = (el: HTMLElement, value: string) => fireEvent.change(el, { target: { value } });
const value = (el: HTMLElement) => (el as HTMLInputElement).value;

afterEach(() => vi.unstubAllGlobals());

describe.each([
  ["wide", false],
  ["narrow", true],
])("a form opened %s keeps what was typed through the other layout and back", (_where, startNarrow) => {
  const memory = {
    key: "on-call", title: "On-call rotation", description: "Who is on call", content: "Carol this week.", index_line: "on-call", revision: 1,
    created_at: "2026-11-01T00:00:00Z", updated_at: "2026-11-01T00:00:00Z", locked: false, locked_at: null, locked_by: null,
    added_by: { kind: "unknown" }, changed_by: { kind: "unknown" }, last_used: null,
  };
  const memories = () => {
    const answer: Record<string, Handler> = { "GET /v1/agents/a1/memories": () => json(200, { items: [memory], next_cursor: null }) };
    routes({ "GET /v1/me": () => json(200, me()), ...answer });
  };

  beforeEach(() => window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=memory"));

  it("the Memory editor, for a memory that exists", async () => {
    const resize = screenWidth(startNarrow);
    memories();
    wrap(<MemoryTab agentId="a1" agentName="Daneel" />);
    fireEvent.click(within(await screen.findByRole("list", { name: "Memories" })).getByText("On-call rotation"));
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    await screen.findByRole("form", { name: "Edit On-call rotation" });
    type(screen.getByLabelText("Title"), "Typed title");
    type(screen.getByRole("textbox", { name: "What should Daneel remember?" }), "Typed text");
    for (const narrow of [!startNarrow, startNarrow]) {
      resize(narrow);
      expect(screen.getAllByRole("form")).toHaveLength(1);
      expect(value(screen.getByLabelText("Title"))).toBe("Typed title");
      expect(value(screen.getByRole("textbox", { name: "What should Daneel remember?" }))).toBe("Typed text");
    }
  });

  it("the Memory editor, for a new memory", async () => {
    const resize = screenWidth(startNarrow);
    memories();
    wrap(<MemoryTab agentId="a1" agentName="Daneel" />);
    fireEvent.click(await screen.findByRole("button", { name: "New memory" }));
    type(screen.getByLabelText("Title"), "A new one");
    for (const narrow of [!startNarrow, startNarrow]) {
      resize(narrow);
      expect(value(screen.getByLabelText("Title"))).toBe("A new one");
    }
  });

  it("the skill editor, with its files open and the Add file dialog half filled", async () => {
    window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=skills");
    const resize = screenWidth(startNarrow);
    const summarize = skill({ id: "s1", name: "summarize", description: "Summarize.", body: "Five bullets.", files: [{ path: "template.md", size: 9, executable: false }] });
    routes({
      "GET /v1/me": () => json(200, me()),
      "GET /v1/agents/a1/skills/effective": effective([applied(summarize)]),
      "GET /v1/skills/s1/files/template.md": () => new Response("## Summary"),
    });
    wrap(<AgentSkills />);
    fireEvent.click(await screen.findByRole("button", { name: "summarize" }));
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    await screen.findByRole("form", { name: "Edit summarize" });
    type(screen.getByLabelText("Name"), "Typed name");
    type(screen.getByRole("textbox", { name: "Instructions" }), "Typed instructions");
    fireEvent.click(screen.getByRole("button", { name: /Files · 2/ }));
    fireEvent.click(screen.getByRole("button", { name: "Open template.md" }));
    type(await screen.findByRole("textbox", { name: "Content of template.md" }), "Typed file");
    const kept = () => {
      expect(value(screen.getByLabelText("Name"))).toBe("Typed name");
      expect(value(screen.getByRole("textbox", { name: "Instructions" }))).toBe("Typed instructions");
      expect(value(screen.getByRole("textbox", { name: "Content of template.md" }))).toBe("Typed file");
    };
    for (const narrow of [!startNarrow, startNarrow]) {
      resize(narrow);
      kept();
    }
    fireEvent.click(screen.getByRole("button", { name: "Add file" }));
    type(within(await screen.findByRole("dialog", { name: "Add a file" })).getByLabelText("File name"), "scripts/run.sh");
    resize(!startNarrow);
    expect(value(within(screen.getByRole("dialog", { name: "Add a file" })).getByLabelText("File name"))).toBe("scripts/run.sh");
  });

  it("the skill editor, for a new skill", async () => {
    window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=skills");
    const resize = screenWidth(startNarrow);
    routes({ "GET /v1/me": () => json(200, me()), "GET /v1/agents/a1/skills/effective": effective([]) });
    wrap(<AgentSkills />);
    await screen.findByRole("button", { name: "Add skill" });
    openMenu("Add skill");
    fireEvent.click(screen.getByRole("menuitem", { name: "Write a new skill" }));
    type(await screen.findByLabelText("Name"), "A new skill");
    for (const narrow of [!startNarrow, startNarrow]) {
      resize(narrow);
      expect(value(screen.getByLabelText("Name"))).toBe("A new skill");
    }
  });
});
