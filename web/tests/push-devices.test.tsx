import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { PushDevices } from "@/components/account/push-devices";
import { bytesToBase64Url, hasPushMarker, PUSH_MARKER_KEY, removeThisDevicePush, resyncPush, setPushMarker } from "@/lib/web-push";
import { json } from "./support/api";

const KEY = bytesToBase64Url(new Uint8Array(65).fill(4));
const OTHER_KEY = bytesToBase64Url(new Uint8Array(65).fill(9));

interface Call {
  method: string;
  path: string;
  body: unknown;
}

function row(over: Record<string, unknown> = {}) {
  return {
    id: "s1",
    label: "Chrome on Linux",
    show_details: false,
    paused: false,
    state: "active",
    this_session: true,
    created_at: "2026-11-01T10:00:00Z",
    last_success_at: null,
    last_failure_at: null,
    ...over,
  };
}

function server(opts: { available?: boolean; key?: string; rows?: unknown[]; rebind404?: boolean } = {}) {
  const calls: Call[] = [];
  const state = { rows: opts.rows ?? [] };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" ? "" : await req.clone().text();
      const body = text ? JSON.parse(text) : null;
      calls.push({ method: req.method, path: url.pathname, body });
      const key = `${req.method} ${url.pathname}`;
      if (key === "GET /v1/push/config") {
        return json(200, { available: opts.available ?? true, vapid_public_key: opts.key ?? KEY, allowed_hosts: ["fcm.googleapis.com"] });
      }
      if (key === "GET /v1/me/push-subscriptions") return json(200, { subscriptions: state.rows });
      if (key === "POST /v1/me/push-subscriptions") {
        if (body?.rebind && opts.rebind404) return json(404, { error: { code: "push_subscription_not_found", message: "nf" } });
        const created = row({ id: "new", this_session: true });
        state.rows = [created];
        return json(201, created);
      }
      if (req.method === "DELETE") {
        state.rows = state.rows.filter((r) => (r as { id: string }).id !== url.pathname.split("/").pop());
        return new Response(null, { status: 204 });
      }
      if (req.method === "PATCH") return json(200, row({ ...body }));
      if (key.endsWith("/test")) return json(202, {});
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

interface Browser {
  register: ReturnType<typeof vi.fn>;
  subscribe: ReturnType<typeof vi.fn>;
  unregister: ReturnType<typeof vi.fn>;
  unsubscribe: ReturnType<typeof vi.fn>;
  requestPermission: ReturnType<typeof vi.fn>;
  sub: () => unknown;
}

function browser(
  over: { permission?: string; registered?: boolean; subKey?: string; secure?: boolean; push?: boolean; ua?: string } = {},
): Browser {
  let current: unknown = null;
  const unsubscribe = vi.fn(async () => {
    current = null;
    return true;
  });
  const mkSub = (key: Uint8Array | string) => {
    const bytes = typeof key === "string" ? Uint8Array.from(atob(key.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0)) : key;
    return {
      endpoint: "https://fcm.googleapis.com/send/abc",
      expirationTime: null,
      options: { applicationServerKey: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) },
      toJSON: () => ({ endpoint: "https://fcm.googleapis.com/send/abc", keys: { p256dh: "PUB", auth: "AUTH" } }),
      unsubscribe,
    };
  };
  if (over.registered) current = mkSub(over.subKey ?? KEY);
  const subscribe = vi.fn(async (o: { applicationServerKey: Uint8Array }) => (current = mkSub(o.applicationServerKey)));
  const unregister = vi.fn(async () => true);
  const reg = { pushManager: { subscribe, getSubscription: vi.fn(async () => current) }, unregister };
  const register = vi.fn<(url: string, o: unknown) => Promise<typeof reg>>(async () => reg);
  let registered = over.registered ?? false;
  Object.defineProperty(window, "isSecureContext", { value: over.secure ?? true, configurable: true });
  Object.defineProperty(navigator, "userAgent", { value: over.ua ?? "Mozilla/5.0 (X11; Linux x86_64) Chrome/130.0 Safari/537.36", configurable: true });
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value: {
      register: vi.fn(async (url: string, o: unknown) => {
        registered = true;
        return register(url, o);
      }),
      ready: Promise.resolve(reg),
      getRegistration: vi.fn(async () => (registered ? reg : undefined)),
    },
  });
  if (over.push === false) delete (window as unknown as Record<string, unknown>).PushManager;
  else (window as unknown as Record<string, unknown>).PushManager = function PushManager() {};
  const requestPermission = vi.fn(async () => {
    (window.Notification as unknown as { permission: string }).permission = "granted";
    return "granted";
  });
  vi.stubGlobal("Notification", { permission: over.permission ?? "default", requestPermission });
  return { register: navigator.serviceWorker.register as never, subscribe, unregister, unsubscribe, requestPermission, sub: () => current };
}

function wrap() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <PushDevices />
    </QueryClientProvider>,
  );
}

beforeEach(() => window.localStorage.clear());

describe("Web Push card", () => {
  it("renders nothing while push is not available, and never registers a worker", async () => {
    const b = browser();
    const calls = server({ available: false });
    const { container } = wrap();
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/push/config")).toBe(true));
    expect(container).toBeEmptyDOMElement();
    expect(b.register).not.toHaveBeenCalled();
  });

  it("does not register a worker for a person who has not opted in", async () => {
    const b = browser();
    server();
    wrap();
    await screen.findByRole("button", { name: "Turn on" });
    expect(b.register).not.toHaveBeenCalled();
    expect(b.requestPermission).not.toHaveBeenCalled();
  });

  it("says so when the browser has no Push API", async () => {
    browser({ push: false });
    server();
    wrap();
    expect(await screen.findByText("This browser cannot receive push notifications.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Turn on" })).not.toBeInTheDocument();
  });

  it("says so on an insecure origin", async () => {
    browser({ secure: false });
    server();
    wrap();
    expect(await screen.findByText(/need a secure connection/)).toBeInTheDocument();
  });

  it("shows the Home Screen instruction on iOS outside a standalone app", async () => {
    browser({ ua: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) Safari/604.1", push: false });
    server();
    wrap();
    expect(await screen.findByText(/added to the Home Screen/)).toBeInTheDocument();
  });

  it("says permission is blocked", async () => {
    browser({ permission: "denied" });
    server();
    wrap();
    expect(await screen.findByText(/blocked for this site/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Turn on" })).not.toBeInTheDocument();
  });

  it("Turn on is a gesture: register, ask permission, subscribe, POST, set the marker", async () => {
    const b = browser();
    const calls = server();
    wrap();
    const button = await screen.findByRole("button", { name: "Turn on" });
    expect(b.requestPermission).not.toHaveBeenCalled();
    fireEvent.click(button);
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/v1/me/push-subscriptions")).toBe(true));
    expect(b.register).toHaveBeenCalledWith("/aurora-push-sw.js", { scope: "/" });
    expect(b.requestPermission).toHaveBeenCalledTimes(1);
    const opts = b.subscribe.mock.calls[0][0];
    expect(opts.userVisibleOnly).toBe(true);
    expect(bytesToBase64Url(opts.applicationServerKey)).toBe(KEY);
    const post = calls.find((c) => c.method === "POST")!.body as Record<string, unknown>;
    expect(post).toMatchObject({
      endpoint: "https://fcm.googleapis.com/send/abc",
      keys: { p256dh: "PUB", auth: "AUTH" },
      vapid_public_key: KEY,
      label: "Chrome on Linux",
    });
    expect(hasPushMarker()).toBe(true);
  });

  it("a refused permission unregisters the worker and sets no marker", async () => {
    const b = browser();
    b.requestPermission.mockImplementation(async () => {
      (window.Notification as unknown as { permission: string }).permission = "denied";
      return "denied";
    });
    server();
    wrap();
    fireEvent.click(await screen.findByRole("button", { name: "Turn on" }));
    expect(await screen.findByText(/blocked for this site/)).toBeInTheDocument();
    expect(b.unregister).toHaveBeenCalled();
    expect(b.subscribe).not.toHaveBeenCalled();
    expect(hasPushMarker()).toBe(false);
  });

  it("lists devices with their states", async () => {
    browser({ registered: true });
    setPushMarker(true);
    server({
      rows: [
        row({ id: "a", label: "Laptop" }),
        row({ id: "b", label: "Phone", this_session: false, paused: true, state: "paused" }),
        row({ id: "c", label: "Tablet", this_session: false, state: "disabled" }),
        row({ id: "d", label: "Old", this_session: false, state: "stale_key" }),
      ],
    });
    wrap();
    const laptop = await screen.findByRole("listitem", { name: "Laptop" });
    expect(within(laptop).getByText("This device")).toBeInTheDocument();
    expect(within(laptop).getByText("Active")).toBeInTheDocument();
    expect(within(screen.getByRole("listitem", { name: "Phone" })).getByText("Paused")).toBeInTheDocument();
    expect(within(screen.getByRole("listitem", { name: "Phone" })).getByRole("button", { name: "Resume" })).toBeInTheDocument();
    expect(within(screen.getByRole("listitem", { name: "Tablet" })).getByText("Not receiving")).toBeInTheDocument();
    expect(within(screen.getByRole("listitem", { name: "Old" })).getByText("Needs update")).toBeInTheDocument();
    expect(within(laptop).getByText(/can appear on a lock screen/)).toBeInTheDocument();
  });

  it("Remove confirms, deletes the row, unsubscribes and unregisters the worker", async () => {
    const b = browser({ registered: true });
    setPushMarker(true);
    const calls = server({ rows: [row()] });
    wrap();
    const item = await screen.findByRole("listitem", { name: "Chrome on Linux" });
    fireEvent.click(within(item).getByRole("button", { name: "Remove" }));
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/me/push-subscriptions/s1")).toBe(true));
    await waitFor(() => expect(b.unregister).toHaveBeenCalled());
    expect(b.unsubscribe).toHaveBeenCalled();
    expect(window.localStorage.getItem(PUSH_MARKER_KEY)).toBeNull();
  });

  it("Send test posts to the device's test route", async () => {
    browser({ registered: true });
    setPushMarker(true);
    const calls = server({ rows: [row()] });
    wrap();
    fireEvent.click(await screen.findByRole("button", { name: "Send test" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/me/push-subscriptions/s1/test")).toBe(true));
  });
});

describe("sign-out and re-sync", () => {
  it("sign-out deletes this device's row and unsubscribes", async () => {
    const b = browser({ registered: true });
    setPushMarker(true);
    const calls = server({ rows: [row({ id: "mine" }), row({ id: "other", this_session: false })] });
    await removeThisDevicePush();
    expect(calls.filter((c) => c.method === "DELETE").map((c) => c.path)).toEqual(["/v1/me/push-subscriptions/mine"]);
    expect(b.unsubscribe).toHaveBeenCalled();
    expect(b.unregister).toHaveBeenCalled();
    expect(hasPushMarker()).toBe(false);
  });

  it("sign-out does nothing for a browser that never opted in", async () => {
    browser({ registered: true });
    const calls = server({ rows: [row()] });
    await removeThisDevicePush();
    expect(calls).toEqual([]);
  });

  it("a rotated key re-subscribes with the new key, posts it and deletes the old row", async () => {
    const b = browser({ registered: true, subKey: OTHER_KEY });
    setPushMarker(true);
    const calls = server({ rows: [row({ id: "old" })] });
    await expect(resyncPush()).resolves.toBe("rotated");
    expect(b.unsubscribe).toHaveBeenCalled();
    expect(bytesToBase64Url(b.subscribe.mock.calls[0][0].applicationServerKey)).toBe(KEY);
    const post = calls.find((c) => c.method === "POST")!;
    expect((post.body as { vapid_public_key: string }).vapid_public_key).toBe(KEY);
    expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/me/push-subscriptions/old")).toBe(true);
  });

  it("a browser the server no longer lists for this session is re-bound", async () => {
    browser({ registered: true });
    setPushMarker(true);
    const calls = server({ rows: [row({ this_session: false })] });
    await expect(resyncPush()).resolves.toBe("rebound");
    const post = calls.find((c) => c.method === "POST")!;
    expect((post.body as { rebind: boolean }).rebind).toBe(true);
    expect(hasPushMarker()).toBe(true);
  });

  it("a 404 on re-bind clears the marker so the opt-in card shows again", async () => {
    const b = browser({ registered: true });
    setPushMarker(true);
    server({ rows: [], rebind404: true });
    await expect(resyncPush()).resolves.toBe("cleared");
    expect(hasPushMarker()).toBe(false);
    expect(b.unsubscribe).toHaveBeenCalled();
  });

  it("a current key and a listed session change nothing", async () => {
    const b = browser({ registered: true });
    setPushMarker(true);
    const calls = server({ rows: [row()] });
    await expect(resyncPush()).resolves.toBe("ok");
    expect(calls.filter((c) => c.method !== "GET")).toEqual([]);
    expect(b.subscribe).not.toHaveBeenCalled();
  });

  it("without the marker nothing is fetched at all", async () => {
    browser({ registered: true });
    const calls = server();
    await expect(resyncPush()).resolves.toBe("none");
    expect(calls).toEqual([]);
  });
});
