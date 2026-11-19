import { test } from "@playwright/test";

// P5-WEBHOOK-4 acceptance, against the dev stack with the recorded receiver. Stays skipped (P5-WEBHOOK-2 is merged): every
// case needs the real server and the recorded receiver at ~/.config/aurora-dev/p5-webhook-test-url, and local e2e against
// a dev stack is not run (owner rule). The mocked journeys (presets, examples, edit, preview, dry run, Send test, limit
// line, both themes at 390 px, axe) are `webhook.spec.ts`. Run with AURORA_E2E_WEBHOOK_STACK=1 by whoever owns the stack.
const enabled = process.env.AURORA_E2E_WEBHOOK_STACK === "1";

test.describe("outgoing webhook on the dev stack", () => {
  test.skip(!enabled, "needs the dev stack and the recorded receiver; local dev-stack e2e is not run");

  test("connect with a preset", async () => {
    test.fixme(true, "write against the stack when P5-WEBHOOK-2 is merged: Slack preset, connect, card shows host and limit");
  });
  test("connect from an example", async () => {
    test.fixme(true, "paste an example, pick a value, Use this template, connect");
  });
  test("edit a slot, preview, dry run and a real Send test to the recorded receiver", async () => {
    test.fixme(true, "Edit › description of a slot; Preview; Dry run; Send after confirmation; status and delivery id shown");
  });
  test("the effective limit is shown; both themes at 390 px with no horizontal scroll; axe", async () => {
    test.fixme(true, "the card's limit line and its source; overflow <= 0; no serious axe violation");
  });
});
