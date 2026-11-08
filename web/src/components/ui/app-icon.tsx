"use client";

import { useState } from "react";

import { cn } from "@/lib/utils";

// Fixed sizes use classes (the production CSP blocks inline style attributes).
const SIZES: Record<number, string> = {
  16: "size-4 text-[10px]",
  20: "size-5 text-[11px]",
  24: "size-6 text-xs",
  32: "size-8 text-sm",
  40: "size-10 text-lg",
  48: "size-12 text-xl",
};

/** Same-origin icon route of an app; `variant=dark` is the package's `assets/icon-dark.png`. */
export function appIconUrl(appId: string, dark = false): string {
  const base = `/v1/apps/${encodeURIComponent(appId)}/icon`;
  return dark ? `${base}?variant=dark` : base;
}

/**
 * App or package icon (modelled on `Avatar`): the icon image, and a monogram tile (the name's
 * first letter on a token background, both themes) when the app has no icon or the image fails.
 *
 * In the dark theme the package's dark icon is used when the API says it has one
 * (`has_dark_icon`), because near-black brand glyphs disappear on dark surfaces. Both images are
 * rendered and CSS picks one by `data-theme` (no flash, no theme state); the hidden one is lazy.
 * A failed dark image falls back to the light icon, a failed light image to the monogram.
 */
export function AppIcon({
  appId,
  name,
  hasDarkIcon = false,
  hasIcon = true,
  size = 40,
  className,
}: {
  appId: string;
  name: string;
  hasDarkIcon?: boolean;
  /** `false` when the app is known to ship no icon: the monogram is shown without a request. */
  hasIcon?: boolean;
  size?: number;
  className?: string;
}) {
  const [lightFailed, setLightFailed] = useState(false);
  const [darkFailed, setDarkFailed] = useState(false);
  const sizeClass = SIZES[size] ?? SIZES[40];
  const frame = cn("shrink-0 border border-line", sizeClass, className);
  // Vendor artwork is shown unaltered (AWS terms, plugins/builtin/ICONS.md): the <img> has no
  // radius (a radius would clip the corners), no filter, tint or overlay, and `object-contain`.
  // Only the monogram tile, which is not vendor artwork, is rounded.
  if (!hasIcon || lightFailed) {
    return (
      <span
        aria-hidden="true"
        data-testid="app-icon-fallback"
        className={cn(frame, "inline-flex rounded-control items-center justify-center bg-accent-soft font-semibold uppercase text-accent-ink")}
      >
        {name.trim().charAt(0) || "?"}
      </span>
    );
  }
  const showDark = hasDarkIcon && !darkFailed;
  return (
    <>
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        src={appIconUrl(appId)}
        alt=""
        width={size}
        height={size}
        onError={() => setLightFailed(true)}
        className={cn(frame, "bg-surface object-contain", showDark && "dark:hidden")}
      />
      {showDark ? (
        // eslint-disable-next-line @next/next/no-img-element
        <img
          src={appIconUrl(appId, true)}
          alt=""
          width={size}
          height={size}
          loading="lazy"
          onError={() => setDarkFailed(true)}
          className={cn(frame, "hidden bg-surface object-contain dark:block")}
        />
      ) : null}
    </>
  );
}
