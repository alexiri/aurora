import type { Metadata } from "next";
import localFont from "next/font/local";
import { cookies, headers } from "next/headers";
import type { ReactNode } from "react";

import "./globals.css";
import { RateLimitNotice } from "@/components/rate-limit-notice";
import { isPreference, THEME_COOKIE, THEME_SCRIPT } from "@/components/theme/theme";
import { ThemeSync } from "@/components/theme/theme-switch";
import { VersionBanner } from "@/components/version-banner";
import { parseRatio } from "@/lib/telemetry";
import { Providers } from "./providers";

// Self-hosted variable fonts (I-40): same-origin, no font CDN, CSP unchanged.
const sans = localFont({
  src: "../../node_modules/@fontsource-variable/inter/files/inter-latin-wght-normal.woff2",
  variable: "--font-inter",
  weight: "100 900",
  display: "swap",
});
const display = localFont({
  src: "../../node_modules/@fontsource-variable/space-grotesk/files/space-grotesk-latin-wght-normal.woff2",
  variable: "--font-grotesk",
  weight: "300 700",
  display: "swap",
});
const mono = localFont({
  src: "../../node_modules/@fontsource-variable/jetbrains-mono/files/jetbrains-mono-latin-wght-normal.woff2",
  variable: "--font-jbmono",
  weight: "100 800",
  display: "swap",
});

// Dynamic rendering so the proxy-issued CSP nonce reaches Next's scripts.
export const dynamic = "force-dynamic";

export const metadata: Metadata = {
  title: "Aurora",
  icons: { icon: "/icon.svg", apple: "/apple-icon.png" },
  // (P4-011B-4, C-835) The static manifest: iOS only offers Web Push to a site added to the Home Screen.
  manifest: "/manifest.webmanifest",
};

export default async function RootLayout({ children }: { children: ReactNode }) {
  const nonce = (await headers()).get("x-nonce") ?? undefined;
  const stored = (await cookies()).get(THEME_COOKIE)?.value;
  // Explicit Light/Dark is rendered server-side; System is resolved by the head script (I-41).
  const theme = isPreference(stored) && stored !== "system" ? stored : undefined;
  // (FIX-146, D-232 (i)) The chart renders this only when an OTLP endpoint is set; absent or invalid: no tracing code loads.
  const tracingRatio = parseRatio(process.env.AURORA_TRACING_SAMPLE_RATIO);
  return (
    <html
      lang="en"
      data-theme={theme}
      className={`${sans.variable} ${display.variable} ${mono.variable}`}
      suppressHydrationWarning
    >
      <head>
        {/* I-41: constant theme script, no interpolated data; runs before first paint. */}
        {/* eslint-disable-next-line react/no-danger -- the sole permitted exception (I-41) */}
        <script nonce={nonce} suppressHydrationWarning dangerouslySetInnerHTML={{ __html: THEME_SCRIPT }} />
      </head>
      <body>
        <Providers tracingRatio={tracingRatio}>
          <ThemeSync />
          <VersionBanner />
          {children}
          <RateLimitNotice />
        </Providers>
      </body>
    </html>
  );
}
