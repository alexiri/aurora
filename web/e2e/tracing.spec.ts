import { expect, test, type Page, type Request } from "@playwright/test";

import { AGENT_ROUTE, DEFAULT, MINE, mockChat, openChat } from "./support/chat-mock";
import { mockApi } from "./support/mock-api";

// FIX-146 Part E (D-232 (i), D-253), mocked API. This spec runs in the `tracing` project, whose server has
// AURORA_TRACING_SAMPLE_RATIO=1 (playwright.config.ts); the default server has none, which the second test uses.

test.use({ viewport: { width: 1280, height: 900 } });

const TRACES = /\/v1\/telemetry\/traces$/;
const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i;
const defaultPort = Number(process.env.AURORA_E2E_WEB_PORT ?? "4100");

interface OtlpSpan {
  traceId: string;
  spanId: string;
  parentSpanId?: string;
  name: string;
  attributes: { key: string; value: { stringValue?: string; intValue?: string } }[];
}

function exported(posts: Request[]): OtlpSpan[] {
  return posts.flatMap(
    (r) =>
      (r.postDataJSON() as { resourceSpans: { scopeSpans: { spans: OtlpSpan[] }[] }[] }).resourceSpans.flatMap((rs) =>
        rs.scopeSpans.flatMap((ss) => ss.spans),
      ),
  );
}

async function send(page: Page) {
  const box = page.locator("#chat-input");
  await expect(box).toBeVisible();
  await box.click();
  await page.keyboard.type("canary-typed-text-7a41");
  const sent = page.waitForRequest((r) => r.method() === "POST" && /\/messages$/.test(r.url()));
  await page.keyboard.press("Enter");
  return sent;
}

test("with tracing on, the traceparent of the send is the API-call span, a child of the action span; the export has no ids or text", async ({
  page,
}) => {
  const posts: Request[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && TRACES.test(r.url())) posts.push(r);
  });
  await openChat(page, "light", DEFAULT, `?c=${MINE.id}`);
  const sent = await send(page);
  const tp = /^00-([0-9a-f]{32})-([0-9a-f]{16})-01$/.exec((await sent).headers()["traceparent"] ?? "");
  expect(tp, "the message POST carries a sampled traceparent").not.toBeNull();
  const [, traceId, callSpanId] = tp!;

  // The tracer flushes on a 5 s timer: poll with a deadline.
  await expect.poll(() => exported(posts).some((s) => s.spanId === callSpanId), { timeout: 20_000 }).toBe(true);
  const spans = exported(posts);
  const call = spans.find((s) => s.spanId === callSpanId)!;
  expect(call.name).toBe("POST /v1/conversations/{id}/messages");
  expect(call.traceId).toBe(traceId);
  const action = spans.find((s) => s.name === "action send_message");
  expect(action, "the action span").toBeDefined();
  expect(call.parentSpanId).toBe(action!.spanId);
  expect(action!.traceId).toBe(traceId);
  expect(action!.parentSpanId).toBeUndefined();

  // Allowlist: no id in a span name or attribute, nothing the user typed.
  const dump = JSON.stringify(posts.map((r) => r.postDataJSON()));
  expect(dump).not.toContain("canary-typed-text-7a41");
  for (const s of spans) expect(s.name).not.toMatch(UUID);
  expect(dump.replace(/"(traceId|spanId|parentSpanId)":"[0-9a-f]+"/g, "")).not.toMatch(UUID);
});

test("without the env value nothing is traced: no traceparent is sent and nothing is posted", async ({ page, baseURL }) => {
  // The default dev server of this checkout has no AURORA_TRACING_SAMPLE_RATIO. (A dev server may *fetch* the lazy
  // chunk ahead of time, so "not loaded" is asserted by behaviour here and by the size of the main bundle in
  // `pnpm build`; the tracer's code never runs: no header, no export.)
  test.skip(baseURL === undefined, "needs a baseURL");
  const plain = baseURL!.replace(/:\d+$/, `:${defaultPort}`);
  const posts: string[] = [];
  page.on("request", (r) => {
    if (TRACES.test(r.url())) posts.push(r.url());
  });
  await mockApi(page);
  await mockChat(page, DEFAULT);
  await page.goto(`${plain}${AGENT_ROUTE}?c=${MINE.id}`);
  await page.getByRole("tab", { name: "Chat" }).click();
  await page.waitForLoadState("networkidle");
  const sent = await send(page);
  expect((await sent).headers()["traceparent"]).toBeUndefined();
  // The tracer would flush within 5 s of its first span: wait past that on a real condition.
  await page.waitForTimeout(6_500);
  expect(posts).toEqual([]);
});
