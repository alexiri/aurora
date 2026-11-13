import { expect, type Page, type WebSocketRoute } from "@playwright/test";

import { agent, mockApi, org } from "./mock-api";
import type { Scheme } from "./open";

// Shared by the terminal specs (terminal.spec.ts against `next dev`, terminal.prod.spec.ts against
// the production build): the leg-1 server half (C-619) and the agent page mock.

export interface MockPty {
  /** Bytes the PTY "printed" — sent to the pane on open. */
  output: Uint8Array;
  /** The `pty.resize` controls the pane sent, in order. */
  resizes: { pty_id: number; cols: number; rows: number }[];
}

/** The leg-1 server half: replays the greeting, records the pane's resize controls. */
export function servePty(ws: WebSocketRoute, pty: MockPty) {
  ws.onMessage((message) => {
    if (typeof message === "string") {
      const control = JSON.parse(message) as { kind?: string; pty_id?: number; cols?: number; rows?: number };
      if (control.kind === "pty.resize") {
        pty.resizes.push({ pty_id: control.pty_id ?? -1, cols: control.cols ?? 0, rows: control.rows ?? 0 });
      }
      return;
    }
    // Binary keystrokes are accepted and dropped (the mock shell does not echo).
  });
  if (pty.output.length > 0) ws.send(Buffer.from(pty.output));
}

/** The shared mock plus a leg-1 terminal WebSocket at `/v1/agents/{id}/terminal`. */
export async function mockWithTerminal(page: Page, pty: MockPty, over: { frozen?: boolean } = {}) {
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (over.frozen && p.endsWith("/agents")) {
      return json({
        items: [{ ...agent, frozen: { at: "2026-01-02T00:00:00Z", authority: "workspace_owner", by_label: "ada@example.test", level: "agent", reason: "Runaway loop" } }],
        next_cursor: null,
      });
    }
    if (p === `/v1/orgs/${org.id}`) return json(org);
    return route.fallback();
  });
  await page.routeWebSocket(/\/v1\/agents\/[^/]+\/terminal$/, (ws) => servePty(ws, pty));
}

export async function openTab(page: Page, scheme: Scheme = "light", query = "?tab=terminal") {
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`/o/acme/w/main/agents/daneel${query}`);
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await page.waitForLoadState("networkidle");
}

export const enc = (s: string) => new TextEncoder().encode(s);
