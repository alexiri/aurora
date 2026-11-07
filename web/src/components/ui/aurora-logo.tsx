import { useId, type SVGProps } from "react";

import { cn } from "@/lib/utils";

import { FOLD_GAP, RIBBON_OVER, RIBBON_UNDER, STAR, TAGLINE, WORDMARK } from "./aurora-paths";

/*
 * Brand components (D-68, docs/14 §Brand). The brand colours below are the one
 * place hex literals are allowed outside globals.css. The wordmark and tagline
 * are outlines copied from docs/design/ui-refresh/brand/: never typeset them.
 */
const INK = "#0E1525";
const INK_DARK = "#E8ECF3";
const TAGLINE_LIGHT = "#5B6576";
const TAGLINE_DARK = "#B4BDCC";

export type MarkVariant = "auto" | "light" | "dark" | "mono-ink" | "mono-white";

type SvgProps = Omit<SVGProps<SVGSVGElement>, "viewBox" | "children">;

function Ribbon({ ga, gb, star, mono, mask }: { ga: string; gb: string; star: string; mono?: string; mask?: string }) {
  if (mono) {
    return (
      <>
        <path d={RIBBON_UNDER} fill={mono} mask={`url(#${mask})`} />
        <path d={RIBBON_OVER} fill={mono} />
        <path d={STAR} fill={mono} />
      </>
    );
  }
  return (
    <>
      <path d={RIBBON_UNDER} fill={`url(#${ga})`} />
      <path d={RIBBON_OVER} fill={`url(#${gb})`} />
      <path d={STAR} className={star === "auto" ? "fill-ink" : undefined} fill={star === "auto" ? undefined : star} />
    </>
  );
}

function Defs({ ga, gb, mask }: { ga: string; gb: string; mask: string }) {
  return (
    <defs>
      <linearGradient id={ga} x1="0" y1="1" x2="1" y2="0">
        <stop offset="0" stopColor="#117673" />
        <stop offset=".7" stopColor="#0B7267" />
        <stop offset="1" stopColor="#066B5E" />
      </linearGradient>
      <linearGradient id={gb} x1="0" y1="0" x2="1" y2="0">
        <stop offset="0" stopColor="#3ED0B7" />
        <stop offset=".2" stopColor="#5AD8C4" />
        <stop offset=".45" stopColor="#8BE7D7" />
        <stop offset="1" stopColor="#94EBD9" />
      </linearGradient>
      <mask id={mask} maskUnits="userSpaceOnUse" x="0" y="0" width="280" height="152">
        <rect width="280" height="152" fill="#fff" />
        <path d={FOLD_GAP} fill="none" stroke="#000" strokeWidth="10" />
      </mask>
    </defs>
  );
}

function useIds() {
  const base = useId().replace(/:/g, "");
  return { ga: `${base}a`, gb: `${base}b`, mask: `${base}m` };
}

function starFor(variant: MarkVariant) {
  return variant === "light" ? INK : variant === "dark" ? INK_DARK : "auto";
}

function monoFor(variant: MarkVariant) {
  return variant === "mono-ink" ? INK : variant === "mono-white" ? "#FFFFFF" : undefined;
}

/** The twisted-ribbon "A" mark alone. `auto` follows the theme (star only). */
export function AuroraMark({
  variant = "auto",
  title,
  ...props
}: SvgProps & { variant?: MarkVariant; title?: string }) {
  const ids = useIds();
  return (
    <svg
      viewBox="0 0 280 152"
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      focusable="false"
      {...props}
    >
      <Defs {...ids} />
      <Ribbon {...ids} star={starFor(variant)} mono={monoFor(variant)} />
    </svg>
  );
}

/**
 * Mark + outlined wordmark. `horizontal` is the sidebar lockup; `stacked` adds
 * the tagline (auth brand panel only). `variant` defaults to following the theme.
 */
export function AuroraLogo({
  layout = "horizontal",
  variant = "auto",
  className,
  ...props
}: SvgProps & { layout?: "horizontal" | "stacked"; variant?: "auto" | "light" | "dark" }) {
  const ids = useIds();
  const auto = variant === "auto";
  const word = variant === "light" ? INK : variant === "dark" ? INK_DARK : undefined;
  const tag = variant === "dark" ? TAGLINE_DARK : TAGLINE_LIGHT;
  const wordClass = auto ? "fill-ink" : undefined;
  const star = starFor(variant);
  if (layout === "stacked") {
    return (
      <svg
        viewBox="0 0 400 250"
        role="img"
        aria-label="Aurora"
        className={className}
        {...props}
      >
        <Defs {...ids} />
        <svg x="100" y="10" width="200" height="108.57" viewBox="0 0 280 152">
          <Ribbon {...ids} star={star} />
        </svg>
        <path transform="scale(0.83229258,1.2015006)" className={wordClass} fill={word} d={WORDMARK} />
        <path
          transform="scale(0.95318072,1.049119)"
          className={auto ? "fill-muted" : undefined}
          fill={auto ? undefined : tag}
          d={TAGLINE}
        />
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 236 64" role="img" aria-label="Aurora" className={cn(className)} {...props}>
      <Defs {...ids} />
      <svg x="0" y="12" width="74" height="40.17" viewBox="0 0 280 152">
        <Ribbon {...ids} star={star} />
      </svg>
      <g transform="translate(90 20) scale(0.57485) translate(-74.0 -145.25)">
        <path transform="scale(0.83229258,1.2015006)" className={wordClass} fill={word} d={WORDMARK} />
      </g>
    </svg>
  );
}
