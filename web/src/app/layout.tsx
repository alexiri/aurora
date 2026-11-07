import type { Metadata } from "next";
import type { ReactNode } from "react";

import "./globals.css";
import { VersionBanner } from "@/components/version-banner";
import { Providers } from "./providers";

// Dynamic rendering so the proxy-issued CSP nonce reaches Next's scripts.
export const dynamic = "force-dynamic";

export const metadata: Metadata = { title: "Aurora" };

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body>
        <Providers>
          <VersionBanner />
          {children}
        </Providers>
      </body>
    </html>
  );
}
