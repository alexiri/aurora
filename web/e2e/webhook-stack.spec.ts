import { test } from "@playwright/test";

// P5-WEBHOOK-4 acceptance, against the dev stack with the recorded receiver. Written but skipped: it needs P5-WEBHOOK-2 (the
// API) and its receiver at ~/.config/aurora-dev/p5-webhook-test-url. Run with AURORA_E2E_WEBHOOK_STACK=1 once -2 is merged.
// Not run — needs P5-WEBHOOK-2.
const enabled = process.env.AURORA_E2E_WEBHOOK_STACK === "1";

test.describe("outgoing webhook on the dev stack", () => {
  test.skip(!enabled, "not run — needs P5-WEBHOOK-2");

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
