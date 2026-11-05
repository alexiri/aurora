import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "standalone",
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
