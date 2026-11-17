import { expect, test, type CDPSession, type Page } from "@playwright/test";
import { generateKeyPairSync } from "node:crypto";

import { id, mockApi, now } from "./support/mock-api";

// P4-011B-4 (C-835, C-836, SEC-102): the Web Push opt-in against mocked API routes. No service worker exists for a
// person who has not opted in; after Turn-on (notification permission granted in the context, `POST` mocked) the
// browser delivers a push through CDP (`ServiceWorker.deliverPushMessage`), the worker shows its fixed-text
// notification and a click lands on the Approvals page with the approval focused; removing the device unregisters it.

const jwk = generateKeyPairSync("ec", { namedCurve: "prime256v1" }).publicKey.export({ format: "jwk" });
const VAPID = Buffer.concat([Buffer.from([4]), Buffer.from(jwk.x as string, "base64url"), Buffer.from(jwk.y as string, "base64url")]).toString("base64url");

const row = (over: Record<string, unknown> = {}) => ({
  id: id(700),
  label: "Chromium on Linux",
  show_details: false,
  paused: false,
  state: "active",
  this_session: true,
  created_at: now,
  last_success_at: null,
  last_failure_at: null,
  ...over,
});

async function setup(page: Page, opts: { rows?: unknown[] } = {}) {
  const calls: { method: string; path: string; body: unknown }[] = [];
  let rows: unknown[] = opts.rows ?? [];
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (p === "/v1/push/config") return json({ available: true, vapid_public_key: VAPID, allowed_hosts: ["fcm.googleapis.com"] });
    if (p === "/v1/me/push-subscriptions") {
      if (req.method() === "GET") return json({ subscriptions: rows });
      if (req.method() === "POST") {
        calls.push({ method: "POST", path: p, body: req.postDataJSON() });
        rows = [row()];
        return json(rows[0], 201);
      }
    }
    if (p.startsWith("/v1/me/push-subscriptions/") && req.method() === "DELETE") {
      calls.push({ method: "DELETE", path: p, body: null });
      rows = [];
      return route.fulfill({ status: 204 });
    }
    return route.fallback();
  });
  return calls;
}

// The test browser has no push service (`pushManager.subscribe()` fails with a permission error whatever the context
// grants), so the browser's subscription is stood in for by a fake that lives in `sessionStorage`. Everything else is
// real: the page registers the real worker, the browser runs it, and CDP delivers the push to it.
async function fakePushSubscription(page: Page) {
  await page.addInitScript(() => {
    const make = (key: ArrayBuffer) => ({
      endpoint: "https://fcm.googleapis.com/fcm/send/e2e",
      expirationTime: null,
      options: { applicationServerKey: key },
      toJSON: () => ({ endpoint: "https://fcm.googleapis.com/fcm/send/e2e", keys: { p256dh: "e2e-p256dh", auth: "e2e-auth" } }),
      unsubscribe: async () => {
        sessionStorage.removeItem("fake-sub");
        return true;
      },
    });
    PushManager.prototype.subscribe = async function (o: PushSubscriptionOptionsInit) {
      const k = o.applicationServerKey as Uint8Array;
      sessionStorage.setItem("fake-sub", btoa(String.fromCharCode(...k)));
      return make(k.buffer.slice(k.byteOffset, k.byteOffset + k.byteLength) as ArrayBuffer) as unknown as PushSubscription;
    };
    PushManager.prototype.getSubscription = async function () {
      const s = sessionStorage.getItem("fake-sub");
      return s ? (make(Uint8Array.from(atob(s), (c) => c.charCodeAt(0)).buffer) as unknown as PushSubscription) : null;
    };
  });
}

const registrations = (page: Page) => page.evaluate(async () => (await navigator.serviceWorker.getRegistrations()).length);

// New headless Chromium: the default headless shell has no Notifications at all (`Notification.permission` is always
// "denied" there, whatever the context grants).
test.use({ channel: "chromium" });

test.describe("Web Push opt-in", () => {
  test("a person who has not opted in has no service worker, and the card offers Turn on", async ({ page, context, baseURL }) => {
    await context.grantPermissions(["notifications"], { origin: baseURL });
    await setup(page);
    await page.goto("/account");
    await expect(page.getByRole("button", { name: "Turn on" })).toBeVisible();
    await page.waitForLoadState("networkidle");
    expect(await registrations(page)).toBe(0);
  });

  test("Turn on, a delivered push shows the fixed-text notification, a click opens the Approvals page, Remove unregisters", async ({ page, context, baseURL }) => {
    await context.grantPermissions(["notifications"], { origin: baseURL });
    await fakePushSubscription(page);
    const calls = await setup(page);
    await page.goto("/account");
    await page.getByRole("button", { name: "Turn on" }).click();
    await expect(page.getByText("Notifications are on for this device.")).toBeVisible({ timeout: 20_000 });
    expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({ vapid_public_key: VAPID, endpoint: "https://fcm.googleapis.com/fcm/send/e2e" });
    expect(await registrations(page)).toBe(1);

    // Deliver a push through the browser's own service-worker plumbing.
    const cdp: CDPSession = await context.newCDPSession(page);
    const registrationIds: string[] = [];
    cdp.on("ServiceWorker.workerRegistrationUpdated", (e) => registrationIds.push(...e.registrations.map((r: { registrationId: string }) => r.registrationId)));
    await cdp.send("ServiceWorker.enable");
    await expect.poll(() => registrationIds.length).toBeGreaterThan(0);
    // A page loaded after the opt-in is controlled by the worker (the worker never claims the ones before).
    await page.reload();
    await expect(page.getByText("Notifications are on for this device.")).toBeVisible();
    await expect.poll(() => page.evaluate(() => navigator.serviceWorker.controller !== null)).toBe(true);
    const approvalId = id(60); // the approval the mocked Approvals page lists
    // Headless Chromium keeps one notification at a time and drops it at once, so what the worker shows is recorded
    // at the call (the worker is the real one; only `showNotification` is wrapped to note its arguments).
    const worker = context.serviceWorkers()[0];
    await worker.evaluate(() => {
      const w = self as unknown as { __shown: unknown[] };
      w.__shown = [];
      const orig = ServiceWorkerRegistration.prototype.showNotification;
      ServiceWorkerRegistration.prototype.showNotification = function (title: string, options?: NotificationOptions) {
        w.__shown.push({ title, ...options });
        return orig.call(this, title, options);
      };
    });
    await cdp.send("ServiceWorker.deliverPushMessage", {
      origin: new URL(page.url()).origin,
      registrationId: registrationIds[0],
      data: JSON.stringify({ v: 1, type: "approval.pending", approval_id: approvalId, parked: true, url: `/o/acme/w/main/approvals?focus=${approvalId}` }),
    });
    const shown = () => worker.evaluate(() => (self as unknown as { __shown: unknown[] }).__shown);
    await expect.poll(shown).toEqual([
      {
        title: "Aurora — an approval needs you",
        body: "Open Aurora to review it. The agent has stopped waiting.",
        tag: approvalId,
        data: { url: `/o/acme/w/main/approvals?focus=${approvalId}` },
        icon: "/icon-192.png",
        requireInteraction: false,
      },
    ]);

    // A click: show that notification again inside the worker and dispatch a real NotificationEvent at the worker's own handler.
    await worker.evaluate(async () => {
      const self_ = self as unknown as { registration: ServiceWorkerRegistration; __shown: (NotificationOptions & { title: string })[] };
      const { title, ...options } = self_.__shown[0];
      await self_.registration.showNotification(title, options);
      const [n] = await self_.registration.getNotifications();
      const NE = (self as unknown as { NotificationEvent: new (t: string, i: unknown) => Event }).NotificationEvent;
      self.dispatchEvent(new NE("notificationclick", { notification: n, action: "" }));
    });
    await expect(page).toHaveURL(new RegExp(`/o/acme/w/main/approvals\\?focus=${approvalId}$`));
    await expect(page.locator(`#approval-${approvalId}`)).toHaveAttribute("aria-current", "true");

    await page.goto("/account");
    const item = page.getByRole("listitem", { name: "Chromium on Linux" });
    await item.getByRole("button", { name: "Remove" }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Remove" }).click();
    await expect.poll(() => calls.some((c) => c.method === "DELETE")).toBe(true);
    await expect.poll(() => registrations(page)).toBe(0);
  });

  test("the card is absent while push is not available", async ({ page }) => {
    await mockApi(page);
    await page.route("**/v1/push/config", (r) => r.fulfill({ status: 404, contentType: "application/json", body: JSON.stringify({ error: { code: "push_unavailable", message: "off" } }) }));
    await page.goto("/account");
    await expect(page.getByRole("heading", { name: "Approval notifications" })).toBeVisible();
    await page.waitForLoadState("networkidle");
    await expect(page.getByText(/even when Aurora is closed/)).toHaveCount(0);
    expect(await registrations(page)).toBe(0);
  });
});
