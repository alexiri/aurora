import { NextResponse, type NextRequest } from "next/server";

/**
 * Security headers (docs/14 §Architecture rules): per-request CSP nonce,
 * nosniff, same-origin referrer policy, optional host pin.
 * Next 16 renamed `middleware.ts` to `proxy.ts`.
 */
export function proxy(request: NextRequest) {
  const pinned = process.env.AURORA_EXTERNAL_HOST;
  if (pinned && request.headers.get("host") !== pinned) {
    return new NextResponse("Misdirected request", { status: 421 });
  }

  const nonce = Buffer.from(crypto.randomUUID()).toString("base64");
  const dev = process.env.NODE_ENV === "development";
  const csp = [
    "default-src 'self'",
    "img-src 'self' data:",
    // (P4-TERM, SEC-87) `'wasm-unsafe-eval'` lets the Ghostty WASM core compile its module;
    // it grants no script eval. Still nonce'd; dev adds `'unsafe-eval'` for HMR.
    `script-src 'self' 'nonce-${nonce}' 'wasm-unsafe-eval'${dev ? " 'unsafe-eval'" : ""}`,
    // Dev only: Next injects inline styles for HMR/error overlay.
    `style-src 'self'${dev ? " 'unsafe-inline'" : ` 'nonce-${nonce}'`}`,
    "connect-src 'self'",
    "object-src 'none'",
    "base-uri 'self'",
    "form-action 'self'",
    "frame-ancestors 'none'",
  ].join("; ");

  const requestHeaders = new Headers(request.headers);
  requestHeaders.set("x-nonce", nonce);
  requestHeaders.set("Content-Security-Policy", csp);

  const response = NextResponse.next({ request: { headers: requestHeaders } });
  response.headers.set("Content-Security-Policy", csp);
  response.headers.set("X-Content-Type-Options", "nosniff");
  response.headers.set("Referrer-Policy", "same-origin");
  return response;
}

export const config = {
  matcher: [
    {
      source: "/((?!v1/|_next/static|_next/image|favicon.ico).*)",
      missing: [
        { type: "header", key: "next-router-prefetch" },
        { type: "header", key: "purpose", value: "prefetch" },
      ],
    },
  ],
};
