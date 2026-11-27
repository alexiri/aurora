import { expect, test } from "@playwright/test";

import { serious } from "./support/axe";

// After a client-side navigation the old route's <title> is gone before the new one streams in. The
// shared axe helper must analyze the settled document, not that gap, or `document-title` fails at random.
test("serious() waits for the document to be titled before it analyzes the page", async ({ page }) => {
  await page.setContent('<!doctype html><html lang="en"><head></head><body><main><h1>Agents</h1></main></body></html>');
  // The title arrives late, as the streamed metadata does.
  await page.evaluate(() => {
    setTimeout(() => {
      document.title = "Aurora";
    }, 3_000);
  });
  expect(await serious(page)).toEqual([]);
});
