import { expect, test } from "@playwright/test";

import { mkdirSync } from "node:fs";

import { ACCESS_SCREENS, openAccess } from "./support/access";
import { id, mockApi, now, ROUTES } from "./support/mock-api";
import { CONNECTOR_SCREENS, openConnectorScreen } from "./support/connectors";
import { CANVAS, openRoute, overflow } from "./support/open";

// P2-UI stage 4 (docs/18 §Dashboard visual design): on every dashboard route, in both themes,
// the page takes the theme's canvas colour and (at 390 px) has no horizontal page scroll. The
// sidebar background spans the viewport on long pages. The API is mocked with the real built-in
// packages (e2e/support/mock-api.ts).

test.describe("theme and narrow shell, every route (390 px)", () => {
  test.use({ viewport: { width: 390, height: 800 } });

  for (const scheme of ["light", "dark"] as const) {
    for (const r of ROUTES) {
      test(`${r.name}: themed, no horizontal page scroll (${scheme})`, async ({ page }) => {
        await openRoute(page, r.path, scheme);
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor), `${r.path} canvas`).toBe(CANVAS[scheme]);
        expect(await overflow(page), `${r.path} overflows horizontally`).toBeLessThanOrEqual(0);
      });
    }
  }
});

// P4-MCON-3: the in-page screens of Admin → Connectors are themed and never scroll horizontally at
// 390 px; full-page screenshots of the add flow and the detail view for the coordinator.
const SHOTS = "/tmp/aurora-screens/p4-mcon-3";
for (const width of [1280, 390]) {
  test.describe(`connector screens (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      for (const s of CONNECTOR_SCREENS) {
        test(`${s}: themed, no horizontal page scroll (${scheme})`, async ({ page }) => {
          await openConnectorScreen(page, s, scheme);
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page), `${s} overflows horizontally`).toBeLessThanOrEqual(0);
          await page.screenshot({ path: `${SHOTS}/${s}-${scheme}-${width}.png`, fullPage: true });
        });
      }
    }
  });
}

// FIX-022: the agent Access tab (connection list) and the grant editor open, themed, no horizontal page scroll;
// full-page screenshots for the coordinator.
const ACCESS_SHOTS = "/tmp/aurora-screens/fix-022";
mkdirSync(ACCESS_SHOTS, { recursive: true });
for (const width of [1280, 390]) {
  test.describe(`agent Access tab (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      for (const s of ACCESS_SCREENS) {
        test(`${s}: themed, no horizontal page scroll (${scheme})`, async ({ page }) => {
          await openAccess(page, s, scheme);
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page), `${s} overflows horizontally`).toBeLessThanOrEqual(0);
          if (s !== "connections") {
            // The dialog scrolls inside itself; nothing in it may be wider than the dialog.
            const dlg = page.getByRole("dialog");
            expect(await dlg.evaluate((el) => el.scrollWidth - el.clientWidth), `${s} dialog overflows`).toBeLessThanOrEqual(0);
          }
          await page.screenshot({ path: `${ACCESS_SHOTS}/${s}-${scheme}-${width}.png`, fullPage: s === "connections" });
        });
      }
    }
  });
}

// FIX-029 (C-615): the Computer tab's failed notice with **Retry now** and the **Start log**
// section; Admin › Runners' **Start diagnostics** drawer. Themed, no horizontal page scroll;
// full-page screenshots for the coordinator.
const RETRY_SHOTS = "/tmp/aurora-screens/fix-029";
mkdirSync(RETRY_SHOTS, { recursive: true });
test.describe("FIX-029 retry and start-log screens", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  for (const scheme of ["light", "dark"] as const) {
    for (const width of [1280, 390]) {
      test(`agent Computer tab with a failed start (${scheme}, ${width} px)`, async ({ page }) => {
        await mockApi(page, {
          "/v1/agents/.*/sandbox": {
            agent_id: "00000000-0000-4000-8000-000000000011", state: "failed",
            boot_id: "00000000-0000-4000-8000-000000000001", runner_id: null, image: "aurora/agent:latest",
            started_at: null, keep_awake_reasons: [], failure_code: "pod_failed",
            failure_message: "the pod crashed before the supervisor connected", start_attempt: null,
            retryable: true,
            start_log: [
              { ts: now, kind: "phase", phase: "failed", detail: "the pod crashed before the supervisor connected" },
              { ts: now, kind: "phase", phase: "pulling", detail: "pulling aurora/agent:latest" },
              { ts: now, kind: "note", phase: "retry", detail: "Start retried by Ada Lovelace after pod_failed." },
            ],
            processes: [], ports: [], restart_to_update: false,
          },
          "/v1/agents/.*/start-failures": {
            items: [
              { id: id(901), sandbox_id: id(910), agent_id: id(11), code: "pod_failed", message: "the pod crashed before the supervisor connected", phase: "failed", events: [], elapsed_secs: 12, created_at: now },
            ],
          },
          "/v1/agents/.*/sandbox/retry": { status: 409, body: { error: { code: "sandbox_not_failed", message: "not failed" } } },
        });
        await page.setViewportSize({ width, height: 900 });
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto("/o/acme/w/main/agents/daneel?tab=computer");
        await expect(page.getByRole("heading", { name: "Start log" })).toBeVisible();
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
        expect(await overflow(page), "Computer tab overflows horizontally").toBeLessThanOrEqual(0);
        await expect(page.getByRole("button", { name: "Retry now" })).toBeVisible();
        await page.screenshot({ path: `${RETRY_SHOTS}/computer-failed-${scheme}-${width}.png`, fullPage: true });
      });
    }
    test(`admin runners Start diagnostics drawer (${scheme})`, async ({ page }) => {
      await mockApi(page, {
        "/v1/admin/runners": {
          items: [
            { id: id(21), name: "runner-a", installation_fingerprint: "abcdef0123456789", backend: "kubernetes", capabilities: null, last_registered_at: now, created_at: now, live_replicas: 1, sandboxes: 2, running_sandboxes: 1, max_sandboxes: 4, replicas: [] },
          ],
        },
        "/v1/admin/runners/.*/start-failures": {
          items: [
            { id: id(901), sandbox_id: id(910), agent_id: id(11), runner_id: id(21), code: "pod_failed", message: "the pod crashed before the supervisor connected", phase: "failed", events: [], elapsed_secs: 12, created_at: now },
          ],
        },
        "/v1/admin/agents/.*/start-failures": {
          items: [
            { id: id(902), sandbox_id: id(910), agent_id: id(11), runner_id: id(21), code: "pod_failed", message: "the pod crashed before the supervisor connected", phase: "failed", events: [], elapsed_secs: 12, created_at: now },
            { id: id(903), sandbox_id: id(911), agent_id: id(11), runner_id: null, code: "supervisor_unreachable", message: "no hello within 60 s", phase: "booting", events: [], elapsed_secs: 64, created_at: now },
          ],
        },
      });
      await page.setViewportSize({ width: 1280, height: 900 });
      await page.emulateMedia({ colorScheme: scheme });
      await page.goto("/admin/runners");
      await expect(page.getByRole("heading", { name: "runner-a" })).toBeVisible();
      await page.getByRole("button", { name: "Start failures" }).click();
      await page.getByText(id(11)).first().click();
      const dlg = page.getByRole("dialog");
      await expect(dlg).toBeVisible();
      expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
      await page.screenshot({ path: `${RETRY_SHOTS}/start-diagnostics-${scheme}.png`, fullPage: true });
    });
  }
});

test.describe("sidebar background", () => {
  test.use({ viewport: { width: 1280, height: 720 } });

  // P2-UI-3b's full-page screenshots showed the sidebar background ending partway down. Check it with
  // real scrolling: the sticky sidebar must keep covering the whole viewport on a page taller than it.
  for (const scheme of ["light", "dark"] as const) {
    test(`covers the viewport height while a long page scrolls (${scheme})`, async ({ page }) => {
      await openRoute(page, "/o/acme/storage", scheme);
      await page.evaluate(() => {
        const filler = document.createElement("div");
        filler.className = "h-[3000px]";
        document.querySelector("main")!.appendChild(filler);
      });
      for (const y of [0, 900, 2500]) {
        await page.evaluate((top) => window.scrollTo(0, top), y);
        const box = await page.getByRole("complementary", { name: "Sidebar" }).evaluate((el) => {
          const r = el.getBoundingClientRect();
          return { top: r.top, bottom: r.bottom, vh: window.innerHeight, bg: getComputedStyle(el).backgroundColor };
        });
        expect(box.top, `scrollY ${y}`).toBeCloseTo(0, 0);
        expect(box.bottom, `scrollY ${y}`).toBeGreaterThanOrEqual(box.vh - 1);
        expect(box.bg).not.toBe("rgba(0, 0, 0, 0)");
      }
    });
  }
});
