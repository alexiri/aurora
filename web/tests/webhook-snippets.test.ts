import { createHmac } from "node:crypto";
import { execFileSync } from "node:child_process";
import { describe, expect, it } from "vitest";

import { callSentence, webhookSnippet, type SnippetInput } from "@/lib/webhook-snippets";

const URL_FULL = "https://aurora.example/hooks/abc123";
const SECRET = "s3cr3t-value-that-must-not-appear";
const BODY = '{"event":"deploy","status":"ok"}';
const input: SnippetInput = { url: URL_FULL, kind: "hmac_sha256" };
const CARD: SnippetInput = { prefix: "auw_9xLPEkbX", origin: "https://aurora.example", kind: "hmac_sha256" };

describe("webhook snippets", () => {
  it("every signed snippet names the real address and the real header, and never the secret", () => {
    for (const lang of ["curl", "python", "node"] as const) {
      const s = webhookSnippet(lang, input);
      expect(s).toContain(URL_FULL);
      expect(s).toContain("X-Signature");
      expect(s).not.toContain(SECRET);
    }
    expect(webhookSnippet("curl", input)).toContain("$SECRET");
    expect(webhookSnippet("python", input)).toContain("WEBHOOK_SECRET");
    expect(webhookSnippet("node", input)).toContain("WEBHOOK_SECRET");
  });

  it("on a card the address is $WEBHOOK_URL with a comment naming <origin>/v1/hooks/auw_…, never the prefix as an address", () => {
    const where = "https://aurora.example/v1/hooks/auw_9xLPEkbX…";
    const curl = webhookSnippet("curl", CARD);
    expect(curl.split("\n")[0]).toBe(`# WEBHOOK_URL is the address shown when you created it: ${where}`);
    expect(curl).toContain('curl -X POST "$WEBHOOK_URL" ');
    expect(webhookSnippet("python", CARD)).toContain(`# WEBHOOK_URL is the address shown when you created it: ${where}`);
    expect(webhookSnippet("python", CARD)).toContain('urllib.request.Request(os.environ["WEBHOOK_URL"]');
    expect(webhookSnippet("node", CARD)).toContain(`// WEBHOOK_URL is the address shown when you created it: ${where}`);
    expect(webhookSnippet("node", CARD)).toContain("await fetch(process.env.WEBHOOK_URL, {");
    for (const lang of ["curl", "python", "node"] as const) {
      for (const kind of ["none", "bearer", "hmac_sha256"] as const) {
        const s = webhookSnippet(lang, { ...CARD, kind });
        expect(s).not.toMatch(/(?:POST|Request\(|fetch\()\s*"?auw_/);
        expect(s).toContain("WEBHOOK_URL");
      }
    }
  });

  it("the signing line of a card's curl is the one the receiver checks (same vector)", () => {
    const sigLine = webhookSnippet("curl", CARD).split("\n").find((l) => l.startsWith("SIG="))!;
    const out = execFileSync("bash", ["-c", `BODY='${BODY}'; SECRET='${SECRET}'; ${sigLine}; printf %s "$SIG"`], { encoding: "utf8" });
    expect(out).toBe(createHmac("sha256", SECRET).update(BODY).digest("hex"));
  });

  it("a webhook with its own header name shows it", () => {
    for (const lang of ["curl", "python", "node"] as const) {
      const s = webhookSnippet(lang, { ...input, header: "X-Hub-Signature-256" });
      expect(s).toContain("X-Hub-Signature-256");
      expect(s).not.toContain("X-Signature");
    }
  });

  it("the curl recipe's signature equals crypto's HMAC-SHA256 of the body", () => {
    const curl = webhookSnippet("curl", input);
    const sigLine = curl.split("\n").find((l) => l.startsWith("SIG="))!;
    const out = execFileSync("bash", ["-c", `BODY='${BODY}'; SECRET='${SECRET}'; ${sigLine}; printf %s "$SIG"`], { encoding: "utf8" });
    expect(out).toBe(createHmac("sha256", SECRET).update(BODY).digest("hex"));
  });

  it("the Node snippet signs with the same HMAC (run with fetch stubbed)", async () => {
    const code = webhookSnippet("node", input).replace(/^import .*\n/, "");
    let sent: { headers: Record<string, string>; body: string } | undefined;
    const fetchStub = async (_u: string, init: { headers: Record<string, string>; body: string }) => {
      sent = init;
      return {};
    };
    const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor as new (...a: string[]) => (...a: unknown[]) => Promise<void>;
    await new AsyncFunction("createHmac", "process", "fetch", code)(createHmac, { env: { WEBHOOK_SECRET: SECRET, WEBHOOK_URL: URL_FULL } }, fetchStub);
    expect(sent!.headers["X-Signature"]).toBe(`sha256=${createHmac("sha256", SECRET).update(sent!.body).digest("hex")}`);
  });

  it("bearer and anyone have no signature", () => {
    const bearer = webhookSnippet("curl", { url: URL_FULL, kind: "bearer" });
    expect(bearer).toContain('Authorization: Bearer $SECRET');
    expect(bearer).not.toContain("openssl");
    const anyone = webhookSnippet("python", { url: URL_FULL, kind: "none" });
    expect(anyone).not.toContain("WEBHOOK_SECRET");
    expect(anyone).toContain(URL_FULL);
  });

  it("the sentence under the panel title", () => {
    expect(callSentence("hmac_sha256")).toBe("Send a POST with the body signed in the X-Signature header: sha256= followed by the hex HMAC-SHA256 of the body.");
    expect(callSentence("bearer")).toBe("Send a POST with the header Authorization: Bearer <token>.");
    expect(callSentence("none")).toBe("Send a POST to the address.");
  });
});
