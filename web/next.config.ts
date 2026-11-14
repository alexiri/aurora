import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "standalone",
  // The production-build Playwright project (FIX-098) builds into its own directory so it never
  // clobbers a `next dev` server's `.next`.
  distDir: process.env.AURORA_NEXT_DIST_DIR ?? ".next",
  // FIX-048: "All organizations" links /admin, whose first page is Setup. Redirecting in the router
  // (one hop, no render of /admin) instead of in a page's `redirect()` saves a full dynamic render of
  // `/admin` before `/admin/setup` is even requested; on a loaded dev server those two sequential
  // renders took longer than the e2e's 5 s (fix-027-selectors.spec.ts).
  async redirects() {
    return [{ source: "/admin", destination: "/admin/setup", permanent: false }];
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
