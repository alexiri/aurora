import { cn } from "@/lib/utils";

const TONES = {
  accent: "fill-accent",
  "accent-ink": "fill-accent-ink",
  pending: "fill-pending-ink",
  danger: "fill-danger-ink",
} as const;

export type MeterTone = keyof typeof TONES;

/**
 * Horizontal fill bar. The fill is an SVG `<rect width>` attribute, never an inline style
 * attribute, because the production CSP (`style-src 'self' 'nonce-…'`) blocks inline styles. The caller
 * passes the ARIA role and values (`progressbar` or `meter`) and a text label elsewhere, so the
 * state never depends on colour alone.
 */
export function Meter({
  ratio,
  tone = "accent",
  className,
  ...aria
}: {
  /** Filled share, 0 to 1 (clamped). */
  ratio: number;
  tone?: MeterTone;
  className?: string;
  role?: "progressbar" | "meter";
  "aria-label"?: string;
  "aria-valuemin"?: number;
  "aria-valuemax"?: number;
  "aria-valuenow"?: number;
}) {
  const pct = Number.isFinite(ratio) ? Math.max(0, Math.min(1, ratio)) * 100 : 0;
  return (
    <div {...aria} className={cn("h-2 w-full overflow-hidden rounded-full bg-subtle", className)}>
      <svg viewBox="0 0 100 1" preserveAspectRatio="none" aria-hidden="true" focusable="false" className="block h-full w-full">
        <rect width={pct} height="1" className={TONES[tone]} />
      </svg>
    </div>
  );
}
