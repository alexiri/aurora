import type { NextConfig } from "next";

import { STATIC_REDIRECTS } from "./src/lib/legacy-routes";

const nextConfig: NextConfig = {
  output: "standalone",
  // The production-build Playwright project (FIX-098) builds into its own directory so it never
  // clobbers a `next dev` server's `.next`.
  distDir: process.env.AURORA_NEXT_DIST_DIR ?? ".next",
  // C-960 (P5-IA-1): every old route lands on its topic page and tab. Redirecting in the router (one hop, nothing
  // rendered first) answers the paths whose destination does not depend on `?tab=`; Next carries the request's own
  // query through. The `?tab=` maps are a client redirect (`LegacyGate`) on the pages that keep an old path.
  async redirects() {
    return STATIC_REDIRECTS.map((r) => ({ ...r, permanent: false }));
  },
  async rewrites() {
    // Dev only: in production the edge proxy routes /v1/* to the API.
    if (process.env.NODE_ENV === "production") return [];
    return [
      {
        source: "/v1/:path*",
        destination: `${process.env.AURORA_API_URL ?? "http://127.0.0.1:4101"}/v1/:path*`,
      },
    ];
  },
};

export default nextConfig;
