import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CommandPalette, openPalette } from "@/components/palette/command-palette";
import { ShortcutsHost } from "@/components/shortcuts/shortcuts-host";
import { requestVerb, resetVerbs, useVerb } from "@/lib/palette-verb";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-19: the palette (nouns for everyone, verbs behind "Show more controls"), the shortcuts and their guards.

/** `@testing-library/user-event` is not installed: the few gestures these tests need, on `fireEvent`. */
const userEvent = {
  async type(el: HTMLElement, text: string) {
    el.focus();
    fireEvent.change(el, { target: { value: (el as HTMLInputElement).value + text } });
  },
  async clear(el: HTMLElement) {
    fireEvent.change(el, { target: { value: "" } });
  },
  async click(el: HTMLElement) {
    el.focus();
    fireEvent.click(el);
  },
  /** "{Enter}" / "{ArrowDown}" or plain characters, one keydown each, on the focused element. */
  async keyboard(keys: string) {
    const parts = keys.match(/\{[^}]+\}|./g) ?? [];
    for (const p of parts) fireEvent.keyDown(document.activeElement ?? document.body, { key: p.startsWith("{") ? p.slice(1, -1) : p });
  },
};

const push = vi.fn();
vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops", handle: "daneel" }),
  usePathname: () => "/o/acme/w/ops/agents/daneel",
  useRouter: () => ({ push, replace: vi.fn() }),
}));

beforeEach(() => {
  push.mockReset();
  vi.stubGlobal("matchMedia", (q: string) => ({ matches: q === "(hover: hover)", addEventListener: () => {}, removeEventListener: () => {} }));
});
afterEach(() => {
  cleanup();
  // A dialog a test appended to the body by hand (the "dialog open" guard) must not leak into the next test.
  document.querySelectorAll("[role='dialog']").forEach((el) => el.remove());
  vi.unstubAllGlobals();
  resetVerbs();
});

/** `mockFetch` for the generated client (a `Request`) and for the plain `fetch("/v1/me")` of `useMe` (a string). */
function mockAllFetch(routes: Parameters<typeof mockFetch>[0]) {
  const fn = vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
    const h = routes[`${req.method} ${new URL(req.url).pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

function routes() {
  return mockAllFetch({
    "GET /v1/me": () =>
      json(200, {
        user: { id: "u1", display_name: "Ada", email: "ada@example.com", is_instance_admin: false },
        memberships: [
          { org: { id: "o1", slug: "acme", name: "Acme" }, role: "owner", status: "active", workspaces: [{ slug: "ops", name: "Operations", my_role: "owner" }] },
          { org: { id: "o2", slug: "globex", name: "Globex" }, role: "member", status: "active", workspaces: [{ slug: "main", name: "Main", my_role: "member" }] },
        ],
      }),
    "GET /v1/resolve": () => json(200, { org: { id: "o1", slug: "acme", name: "Acme" }, workspace: { id: "w1", slug: "ops", name: "Operations" } }),
    "GET /v1/workspaces/w1": () => json(200, { id: "w1", org_id: "o1", slug: "ops", name: "Operations", my_role: "owner" }),
    "GET /v1/workspaces/w1/agents": () =>
      json(200, {
        items: [
          { id: "a1", name: "Daneel", handle: "daneel", kind: "hosted", frozen: null },
          { id: "a2", name: "Giskard", handle: "giskard", kind: "hosted", frozen: null },
        ],
        next_cursor: null,
      }),
    "GET /v1/instance": () => json(200, { features: { terminal: false, ssh: false, previews: false } }),
    "GET /v1/orgs/o1": () => json(200, { id: "o1", my_role: "owner", settings: {} }),
    "GET /v1/orgs/o1/workspaces": () => json(200, { items: [{ id: "w1", slug: "ops", name: "Operations", my_role: "owner" }], next_cursor: null }),
  });
}

function open() {
  act(() => openPalette());
  return screen.findByRole("combobox", { name: "Search places and actions" });
}

describe("the palette with Show more controls off", () => {
  it("opens on Ctrl K, lists the nouns and no verbs, and goes where a choice says", async () => {
    routes();
    wrap(<><CommandPalette /><ShortcutsHost /></>, { moreControls: false });
    fireEvent.keyDown(document, { key: "k", ctrlKey: true });
    const field = await screen.findByRole("combobox", { name: "Search places and actions" });
    expect(field).toHaveFocus();
    await userEvent.type(field, "dan");
    const list = await screen.findByRole("listbox");
    expect(await within(list).findByRole("option", { name: /Daneel/ })).toBeInTheDocument();
    expect(within(list).queryByRole("option", { name: /Pause/ })).toBeNull();
    expect(screen.queryByText("Actions")).toBeNull();
    await userEvent.keyboard("{Enter}");
    expect(push).toHaveBeenCalledWith("/o/acme/w/ops/agents/daneel");
  });

  it("says what matched nothing", async () => {
    routes();
    wrap(<CommandPalette />, { moreControls: false });
    const field = await open();
    await userEvent.type(field, "zzzz");
    expect(await screen.findByText("Nothing matches “zzzz”.")).toBeInTheDocument();
  });

  it("finds workspaces and organizations and moves with the arrow keys", async () => {
    routes();
    wrap(<CommandPalette />, { moreControls: false });
    const field = await open();
    await screen.findByRole("option", { name: /Operations/ });
    await userEvent.type(field, "glob");
    // The workspace "Main" of Globex. Ada is only a member there, so no entry opens Manage Globex (D-508(e)).
    const hits = await screen.findAllByRole("option", { name: /Globex/ });
    expect(hits.map((h) => h.textContent)).toEqual(["MainGlobex"]);
    expect(hits[0]).toHaveAttribute("aria-selected", "true");
    // Ada owns Acme: its Manage is a place (D-508(f)). She is not an instance admin: no Administration.
    await userEvent.clear(field);
    await userEvent.type(field, "manage");
    // ("manage" also finds the open agent's Manage page.)
    await waitFor(() => expect(screen.getAllByRole("option").map((h) => h.textContent)).toContain("Manage Acme"));
    expect(screen.queryByRole("option", { name: /Manage Globex/ })).toBeNull();
    await userEvent.clear(field);
    await userEvent.type(field, "administration");
    expect(await screen.findByText(/Nothing matches/)).toBeInTheDocument();
    await userEvent.clear(field);
    await userEvent.type(field, "o");
    await waitFor(() => expect(screen.getAllByRole("option").length).toBeGreaterThan(1));
    const before = screen.getAllByRole("option").findIndex((o) => o.getAttribute("aria-selected") === "true");
    await userEvent.keyboard("{ArrowDown}");
    const after = screen.getAllByRole("option").findIndex((o) => o.getAttribute("aria-selected") === "true");
    expect(after).toBe(before + 1);
  });

  it("does not react to the letter shortcuts", async () => {
    routes();
    wrap(<ShortcutsHost />, { moreControls: false });
    await screen.findByRole("button", { name: "Close" }).catch(() => undefined);
    fireEvent.keyDown(document.body, { key: "g" });
    fireEvent.keyDown(document.body, { key: "a" });
    expect(push).not.toHaveBeenCalled();
  });
});

describe("the palette with Show more controls on", () => {
  it("lists the verbs the role allows, and Pause opens the page's dialog instead of pausing", async () => {
    const fetchMock = routes();
    wrap(<CommandPalette />);
    const field = await open();
    await userEvent.type(field, "pause");
    const option = await screen.findByRole("option", { name: /Pause Daneel/ });
    expect(screen.getByText("Actions")).toBeInTheDocument();
    await userEvent.click(option);
    expect(push).toHaveBeenCalledWith("/o/acme/w/ops/agents/daneel?tab=manage");
    // It acted on nothing: no write was sent.
    const writes = fetchMock.mock.calls.filter(([r]) => (r as Request).method !== "GET");
    expect(writes.filter(([r]) => /freeze|pause/.test((r as Request).url))).toEqual([]);
  });

  it("hands the verb to the control that owns the dialog, at once or when it mounts", async () => {
    routes();
    const opened = vi.fn();
    function Owner() {
      const [show, setShow] = useState(false);
      return (
        <>
          <button onClick={() => setShow(true)}>mount</button>
          {show ? <Mounted /> : null}
        </>
      );
    }
    function Mounted() {
      useVerb("new-schedule", opened);
      return null;
    }
    wrap(<Owner />);
    // asked before the page is there: waits, then opens on mount
    requestVerb("new-schedule");
    expect(opened).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "mount" }));
    expect(opened).toHaveBeenCalledTimes(1);
    // asked again while mounted: opens at once
    requestVerb("new-schedule");
    expect(opened).toHaveBeenCalledTimes(2);
  });

  it("has Keyboard shortcuts as a verb that opens the sheet", async () => {
    routes();
    wrap(<><CommandPalette /><ShortcutsHost /></>);
    const field = await open();
    await userEvent.type(field, "keyboard");
    await userEvent.click(await screen.findByRole("option", { name: /Keyboard shortcuts/ }));
    const sheet = await screen.findByTestId("shortcut-sheet");
    expect(within(sheet).getByText("Go to Agents")).toBeInTheDocument();
    expect(push).not.toHaveBeenCalled();
  });
});

describe("shortcuts with Show more controls on", () => {
  const press = (key: string, target: Element | Document = document.body) => fireEvent.keyDown(target, { key });

  it("g a, g p, g o and g s go to the topics the role lists", async () => {
    routes();
    wrap(<ShortcutsHost />);
    await waitFor(() => expect(screen.queryByTestId("shortcut-sheet")).toBeNull());
    // wait for the session so the topics are known
    await waitFor(() => {
      press("g");
      press("o");
      expect(push).toHaveBeenCalledWith("/o/acme/w/ops");
    });
    push.mockReset();
    // Approvals and Settings are listed once the workspace role is known (an owner's).
    for (const [k, to] of [["a", "/o/acme/w/ops/agents"], ["p", "/o/acme/w/ops/approvals"], ["s", "/o/acme/w/ops/settings"]] as const) {
      await waitFor(() => {
        push.mockReset();
        press("g");
        press(k);
        expect(push).toHaveBeenLastCalledWith(to);
      });
    }
  });

  it("n starts a new chat with the open agent and ? opens the sheet", async () => {
    routes();
    wrap(<ShortcutsHost />);
    await waitFor(() => {
      press("n");
      expect(push).toHaveBeenCalledWith("/o/acme/w/ops/agents/daneel");
    });
    press("?");
    expect(await screen.findByTestId("shortcut-sheet")).toBeInTheDocument();
  });

  it("never fires while typing in a field, in the terminal or with a dialog open", async () => {
    routes();
    wrap(
      <>
        <ShortcutsHost />
        <input aria-label="Say something" />
        <div data-no-shortcuts="">
          <button>in the terminal</button>
        </div>
      </>,
    );
    // The same keys do work on the page once the session is loaded ...
    await waitFor(() => {
      press("g");
      press("o");
      expect(push).toHaveBeenCalledWith("/o/acme/w/ops");
    });
    push.mockReset();
    // ... and not in a field, in the terminal, or while a dialog other than the sheet is open.
    for (const target of [screen.getByLabelText("Say something"), screen.getByRole("button", { name: "in the terminal" })]) {
      press("g", target);
      press("o", target);
    }
    expect(push).not.toHaveBeenCalled();
    document.body.insertAdjacentHTML("beforeend", "<div role='dialog' aria-label='Some dialog'></div>");
    press("g");
    press("o");
    expect(push).not.toHaveBeenCalled();
  });

  it("types a, b, n and ? in the composer-like field without shortcuts", async () => {
    routes();
    wrap(
      <>
        <ShortcutsHost />
        <textarea aria-label="Message" />
      </>,
    );
    const box = await screen.findByLabelText("Message");
    await userEvent.click(box);
    await userEvent.keyboard("gonab?");
    expect(push).not.toHaveBeenCalled();
    expect(screen.queryByTestId("shortcut-sheet")).toBeNull();
  });

  it("Ctrl K does not open the palette inside the terminal", async () => {
    routes();
    wrap(
      <>
        <CommandPalette />
        <ShortcutsHost />
        <div data-no-shortcuts="">
          <button>in the terminal</button>
        </div>
      </>,
    );
    fireEvent.keyDown(screen.getByRole("button", { name: "in the terminal" }), { key: "k", ctrlKey: true });
    expect(screen.queryByRole("combobox")).toBeNull();
  });

  it("a / b click the Allow once / Block of the card that has the focus", async () => {
    routes();
    const allow = vi.fn();
    const block = vi.fn();
    wrap(
      <>
        <ShortcutsHost />
        <ul>
          <li data-approval-card="" tabIndex={-1} aria-label="card">
            <button data-approval-key="allow-once" onClick={allow}>Allow once</button>
            <button data-approval-key="block" onClick={block}>Block</button>
          </li>
        </ul>
        <button>elsewhere</button>
      </>,
    );
    // not on a card: nothing
    screen.getByRole("button", { name: "elsewhere" }).focus();
    fireEvent.keyDown(document.activeElement!, { key: "a" });
    expect(allow).not.toHaveBeenCalled();
    screen.getByLabelText("card").focus();
    await waitFor(() => {
      fireEvent.keyDown(document.activeElement!, { key: "a" });
      expect(allow).toHaveBeenCalledTimes(1);
    });
    fireEvent.keyDown(document.activeElement!, { key: "b" });
    expect(block).toHaveBeenCalledTimes(1);
  });
});
