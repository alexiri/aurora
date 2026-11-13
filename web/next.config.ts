import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "standalone",
  // The production-build Playwright project (FIX-098) builds into its own directory so it never
  // clobbers a `next dev` server's `.next`.
  distDir: process.env.AURORA_NEXT_DIST_DIR ?? ".next",
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
