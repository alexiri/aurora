// @vitest-environment node
// The push service worker (C-835, SEC-102) in a service-worker-like global shim.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";

const SOURCE = readFileSync(join(__dirname, "../public/aurora-push-sw.js"), "utf8");

interface Shown {
  title: string;
  options: { body: string; tag: string; data: { url: string }; icon: string; requireInteraction: boolean };
}

function load(clientList: { url: string; focus: () => Promise<unknown>; navigate: (u: string) => Promise<unknown> }[] = []) {
  const handlers: Record<string, (e: unknown) => void> = {};
  const shown: Shown[] = [];
  const opened: string[] = [];
  const self = {
    location: { origin: "https://aurora.example" },
    skipWaiting: vi.fn(),
    addEventListener: (type: string, fn: (e: unknown) => void) => {
      handlers[type] = fn;
    },
    registration: {
      showNotification: (title: string, options: Shown["options"]) => {
        shown.push({ title, options });
        return Promise.resolve();
      },
    },
    clients: {
      matchAll: vi.fn(() => Promise.resolve(clientList)),
      openWindow: (u: string) => {
        opened.push(u);
        return Promise.resolve(null);
      },
    },
  };
  new Function("self", SOURCE)(self);
  return { self, handlers, shown, opened };
}

async function push(raw: string | null) {
  const w = load();
  let waited: Promise<unknown> = Promise.resolve();
  w.handlers.push({ data: raw === null ? null : { text: () => raw }, waitUntil: (p: Promise<unknown>) => (waited = p) });
  await waited;
  return w.shown[0];
}

async function click(url: unknown, clients = [] as Parameters<typeof load>[0]) {
  const w = load(clients);
  const close = vi.fn();
  let waited: Promise<unknown> = Promise.resolve();
  w.handlers.notificationclick({ notification: { close, data: { url } }, waitUntil: (p: Promise<unknown>) => (waited = p) });
  await waited;
  expect(close).toHaveBeenCalled();
  return w;
}

const GENERIC = "Aurora — an approval needs you";

describe("push service worker", () => {
  it("registers exactly install, push, notificationclick and pushsubscriptionchange", () => {
    const w = load();
    expect(Object.keys(w.handlers).sort()).toEqual(["install", "notificationclick", "push", "pushsubscriptionchange"]);
    w.handlers.install({});
    expect(w.self.skipWaiting).toHaveBeenCalled();
    expect(() => w.handlers.pushsubscriptionchange({})).not.toThrow();
  });

  it.each([
    ["malformed JSON", "{nope"],
    ["empty data", null],
    ["v: 2", JSON.stringify({ v: 2, type: "approval.pending", approval_id: "x", url: "/o/a/w/b/approvals" })],
    ["unknown type", JSON.stringify({ v: 1, type: "something.else", approval_id: "x" })],
    ["a JSON array", "[1]"],
  ])("shows the generic notification for %s", async (_n, raw) => {
    const s = await push(raw);
    expect(s.title).toBe(GENERIC);
    expect(s.options.body).toBe("Open Aurora to review it.");
    expect(s.options.data.url).toBe("/");
    expect(s.options.icon).toBe("/icon-192.png");
    expect(s.options.requireInteraction).toBe(false);
  });

  it("approval.pending: title, body, tag = approval id, data.url", async () => {
    const s = await push(
      JSON.stringify({ v: 1, type: "approval.pending", approval_id: "ap-1", parked: false, url: "/o/a/w/b/approvals?focus=ap-1" }),
    );
    expect(s.title).toBe(GENERIC);
    expect(s.options.body).toBe("Open Aurora to review it.");
    expect(s.options.tag).toBe("ap-1");
    expect(s.options.data.url).toBe("/o/a/w/b/approvals?focus=ap-1");
  });

  it("a parked approval adds the stopped-waiting sentence", async () => {
    const s = await push(JSON.stringify({ v: 1, type: "approval.pending", approval_id: "ap-2", parked: true, url: "/o/a/w/b/approvals" }));
    expect(s.options.body).toBe("Open Aurora to review it. The agent has stopped waiting.");
  });

  it("the reminder has its own title", async () => {
    const s = await push(JSON.stringify({ v: 1, type: "approval.reminder", approval_id: "ap-3", url: "/o/a/w/b/approvals" }));
    expect(s.title).toBe("Aurora — an approval is still waiting");
  });

  it("names appear only when both are present", async () => {
    const named = await push(
      JSON.stringify({ v: 1, type: "approval.pending", approval_id: "ap-4", agent: "Mia", workspace: "Ops", url: "/o/a/w/b/approvals" }),
    );
    expect(named.title).toBe("Mia in Ops needs an approval");
    const half = await push(JSON.stringify({ v: 1, type: "approval.pending", approval_id: "ap-4", agent: "Mia", url: "/o/a/w/b/approvals" }));
    expect(half.title).toBe(GENERIC);
    expect(half.title).not.toContain("Mia");
  });

  it("digest and test", async () => {
    const d = await push(JSON.stringify({ v: 1, type: "approval.digest", count: 7, url: "/o/a/w/b/approvals" }));
    expect(d.title).toBe("Aurora — 7 approvals waiting");
    expect(d.options.tag).toBe("aurora-digest");
    const t = await push(JSON.stringify({ v: 1, type: "test" }));
    expect(t.title).toBe("Aurora — test notification");
    expect(t.options.body).toBe("Notifications work on this device.");
    expect(t.options.tag).toBe("aurora-test");
  });

  it("admin_attention: fixed text and /admin; the item's title only when the payload carries it (FIX-167, C-1023)", async () => {
    const plain = await push(JSON.stringify({ v: 1, type: "admin_attention", url: "/admin", detail: "SECRET-CANARY" }));
    expect(plain.title).toBe("Aurora — something in Administration needs your decision");
    expect(plain.options.body).toBe("Open Administration to review it.");
    expect(plain.options.tag).toBe("aurora-admin");
    expect(plain.options.data.url).toBe("/admin");
    expect(JSON.stringify(plain)).not.toContain("SECRET-CANARY");
    const named = await push(JSON.stringify({ v: 1, type: "admin_attention", url: "https://evil.example", title: "Mattermost 2.0 needs your approval" }));
    expect(named.title).toBe("Aurora — Mattermost 2.0 needs your approval");
    expect(named.options.data.url, "the worker never trusts the payload's url for this type").toBe("/admin");
  });

  it("schedule_answer: fixed text, the schedule's name only when carried, and the conversation path it was given (P5-SCHED-ONCE, C-835)", async () => {
    const path = "/o/acme/w/main/agents/daneel?c=3f2b8c1e-6a4d-4e0b-9d57-1c2a7e5b9f10";
    const plain = await push(JSON.stringify({ v: 1, type: "schedule_answer", url: path, answer: "SECRET-CANARY" }));
    expect(plain.title).toBe("Aurora — a scheduled task has an answer for you");
    expect(plain.options.body).toBe("Open the conversation to read it.");
    expect(plain.options.tag).toBe("aurora-schedule");
    expect(plain.options.data.url).toBe(path);
    expect(JSON.stringify(plain)).not.toContain("SECRET-CANARY");
    const named = await push(JSON.stringify({ v: 1, type: "schedule_answer", url: path, title: "Water the plants" }));
    expect(named.title).toBe("Aurora — Water the plants");
    expect(named.options.body).toBe("Open the conversation to read it.");
    const stray = await push(JSON.stringify({ v: 1, type: "schedule_answer", url: "https://evil.example/x" }));
    expect(stray.options.data.url, "a path outside Aurora becomes the home page").toBe("/");
  });

  it("peer_approval: fixed text about the wait, the agent's name only when carried, and the approvals path it was given (FIX-252)", async () => {
    const path = "/o/acme/w/main/approvals?focus=3f2b8c1e-6a4d-4e0b-9d57-1c2a7e5b9f10";
    const plain = await push(JSON.stringify({ v: 1, type: "peer_approval", url: path, detail: "SECRET-CANARY" }));
    expect(plain.title).toBe("Aurora — a delegated task is waiting for an approval");
    expect(plain.options.body).toBe("Open the approvals page to review it.");
    expect(plain.options.tag).toBe("aurora-peer-approval");
    expect(plain.options.data.url).toBe(path);
    expect(JSON.stringify(plain)).not.toContain("SECRET-CANARY");
    const named = await push(JSON.stringify({ v: 1, type: "peer_approval", url: path, agent: "Fixer" }));
    expect(named.title).toBe("Aurora — Fixer is waiting for an approval");
    expect(named.options.body).toBe("Open the approvals page to review it.");
    const stray = await push(JSON.stringify({ v: 1, type: "peer_approval", url: "https://evil.example/x" }));
    expect(stray.options.data.url, "a path outside Aurora becomes the home page").toBe("/");
  });

  it("workspace_lifecycle and org_lifecycle: fixed text, the sentence only when carried (FIX-244, D-486)", async () => {
    const ws = await push(JSON.stringify({ v: 1, type: "workspace_lifecycle", url: "/o/acme/w/main" }));
    expect(ws.title).toBe("Aurora — your workspace changed");
    expect(ws.options.body).toBe("Open Aurora for details.");
    expect(ws.options.tag).toBe("aurora-workspace-lifecycle");
    expect(ws.options.data.url).toBe("/o/acme/w/main");
    const org = await push(JSON.stringify({ v: 1, type: "org_lifecycle", url: "/o/acme" }));
    expect(org.title).toBe("Aurora — your organization changed");
    expect(org.options.tag).toBe("aurora-org-lifecycle");
    expect(org.options.data.url).toBe("/o/acme");
    const named = await push(
      JSON.stringify({ v: 1, type: "workspace_lifecycle", url: "/o/acme/w/main", title: "Workspace Main was frozen" }),
    );
    expect(named.title).toBe("Aurora — Workspace Main was frozen");
    const stray = await push(JSON.stringify({ v: 1, type: "org_lifecycle", url: "https://evil.example/x" }));
    expect(stray.options.data.url).toBe("/");
  });

  it("default_store: fixed text, the sentence only when carried (FIX-244, D-486)", async () => {
    const plain = await push(JSON.stringify({ v: 1, type: "default_store", url: "/" }));
    expect(plain.title).toBe("Aurora — the default secret store changed");
    expect(plain.options.body).toBe("Open Aurora for details.");
    expect(plain.options.tag).toBe("aurora-default-store");
    const named = await push(
      JSON.stringify({ v: 1, type: "default_store", url: "/", title: "The default secret store changed" }),
    );
    expect(named.title).toBe("Aurora — The default secret store changed");
  });

  it("template_update: fixed text, the sentence only when carried, and the agents path it was given (FIX-251, D-488(a))", async () => {
    const path = "/o/acme/w/main/agents?needs_update=1";
    const plain = await push(JSON.stringify({ v: 1, type: "template_update", url: path, detail: "SECRET-CANARY" }));
    expect(plain.title).toBe("Aurora — a template update is available");
    expect(plain.options.body).toBe("Open the Agents page to review the updates.");
    expect(plain.options.tag).toBe("aurora-template-update");
    expect(plain.options.data.url).toBe(path);
    expect(JSON.stringify(plain)).not.toContain("SECRET-CANARY");
    const named = await push(
      JSON.stringify({ v: 1, type: "template_update", url: path, title: "T 1.4.0 is available for 3 agents in Main" }),
    );
    expect(named.title).toBe("Aurora — T 1.4.0 is available for 3 agents in Main");
    expect(named.options.body).toBe("Open the Agents page to review the updates.");
    const stray = await push(JSON.stringify({ v: 1, type: "template_update", url: "https://evil.example/x" }));
    expect(stray.options.data.url, "a path outside Aurora becomes the home page").toBe("/");
  });

  it("never shows a field outside the payload contract", async () => {
    const s = await push(
      JSON.stringify({ v: 1, type: "approval.pending", approval_id: "ap-5", title: "SECRET-CANARY", body: "SECRET-CANARY", url: "/o/a/w/b/approvals" }),
    );
    expect(JSON.stringify(s)).not.toContain("SECRET-CANARY");
  });

  it.each([
    ["/o/a/w/b/approvals?focus=x", "/o/a/w/b/approvals?focus=x"],
    ["/o/my.org/w/b/approvals", "/o/my.org/w/b/approvals"],
    ["//evil", "/"],
    ["https://evil", "/"],
    ["javascript:alert(1)", "/"],
    ["/\\evil", "/"],
    ["/o/../admin", "/"],
    ["/o/a/./b", "/"],
    ["/o//a", "/"],
    // (FIX-167, C-1023) Exactly `/admin` is the one non-/o path a notification may open.
    ["/admin", "/admin"],
    ["/admin/people", "/"],
    ["/admin?x=1", "/"],
    ["/administrator", "/"],
    ["/o/a?x=/evil", "/"],
    [undefined, "/"],
    [42, "/"],
  ])("notificationclick %j opens %j", async (url, expected) => {
    const w = await click(url);
    expect(w.opened).toEqual([expected]);
  });

  it("focuses an existing Aurora client and navigates it instead of opening a window", async () => {
    const navigate = vi.fn(() => Promise.resolve());
    const focus = vi.fn(() => Promise.resolve({ navigate }));
    const w = await click("/o/a/w/b/approvals?focus=x", [
      { url: "https://elsewhere.example/", focus: vi.fn(), navigate: vi.fn() },
      { url: "https://aurora.example/o/a", focus, navigate },
    ]);
    expect(focus).toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledWith("/o/a/w/b/approvals?focus=x");
    expect(w.opened).toEqual([]);
  });

  it("navigates even when focus is refused, and opens a window when the client cannot be navigated", async () => {
    const navigate = vi.fn(() => Promise.resolve());
    const w1 = await click("/o/a/w/b/approvals", [{ url: "https://aurora.example/o/a", focus: () => Promise.reject(new Error("no")), navigate }]);
    expect(navigate).toHaveBeenCalledWith("/o/a/w/b/approvals");
    expect(w1.opened).toEqual([]);
    // A tab opened before the opt-in is not controlled by the worker: navigate() rejects.
    const w2 = await click("/o/a/w/b/approvals", [
      { url: "https://aurora.example/o/a", focus: () => Promise.resolve({ navigate: () => Promise.reject(new TypeError("uncontrolled")) }), navigate },
    ]);
    expect(w2.opened).toEqual(["/o/a/w/b/approvals"]);
  });

  it("opens a window when no Aurora client exists", async () => {
    const w = await click("/o/a/w/b/approvals", [{ url: "https://elsewhere.example/", focus: vi.fn(), navigate: vi.fn() }]);
    expect(w.opened).toEqual(["/o/a/w/b/approvals"]);
  });
});

describe("SEC-102 source test", () => {
  const code = SOURCE.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
  it.each([
    ["a fetch listener", /addEventListener\(\s*["']fetch["']/],
    ["Cache Storage", /\bcaches\b/],
    ["importScripts", /importScripts/],
    ["fetch(", /\bfetch\s*\(/],
    ["XMLHttpRequest", /XMLHttpRequest/],
    ["WebSocket", /WebSocket/],
    ["EventSource", /EventSource/],
    ["clients.claim", /clients\s*\.\s*claim/],
    ["an import", /^\s*import\s/m],
  ])("the worker has no %s", (_n, re) => {
    expect(code).not.toMatch(re);
  });
  it("the raw file, comments included, has none of the forbidden words", () => {
    for (const w of ["fetch", "caches", "importScripts", "XMLHttpRequest", "WebSocket", "clients.claim"]) {
      expect(SOURCE).not.toContain(w);
    }
  });
});
