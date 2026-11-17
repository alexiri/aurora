"use client";

import { useEffect, useRef, useState, type KeyboardEvent } from "react";

import { Button } from "@/components/ui/button";
import { compact } from "@/lib/usage-format";

export interface BarDatum {
  key: string;
  label: string;
  value: number;
}

const HEIGHT = 220;
const PAD = { top: 10, right: 8, bottom: 26, left: 44 };
const GAP = 2;
const RADIUS = 4;
const DEFAULT_WIDTH = 600;

/** At most 5 ticks (0 included) with round values up to the data's maximum. */
function ticks(max: number): number[] {
  if (max <= 0) return [0, 1];
  const raw = max / 4;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? raw;
  const out: number[] = [];
  for (let v = 0; v < max + step * 0.999 && out.length < 6; v += step) out.push(Math.round(v * 1000) / 1000);
  return out.length > 5 ? out.slice(0, 5) : out;
}

/** A bar with 4 px rounded top corners, anchored to the baseline. */
function barPath(x: number, y: number, w: number, h: number): string {
  const r = Math.min(RADIUS, w / 2, h);
  return `M${x},${y + h}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h}Z`;
}

/**
 * (C-1005) One series as inline SVG (no chart library): bars on the baseline in `accent`, gridlines in `line`,
 * axis labels in `muted`, at most 5 y ticks with compact numbers, no legend (the heading names the measure). The bar
 * group is one tab stop; the arrow keys, Home and End move between bars and each bar's value shows as a tooltip on
 * hover and on keyboard focus. Every bar has the accessible name "<label>: <value>". It fills its container's width.
 */
export function BarChart({
  data,
  format,
  unitLabel,
  onShowTable,
  width: fixedWidth,
}: {
  data: BarDatum[];
  format: (value: number) => string;
  /** What the bars measure, for the summary ("Tokens per day"). */
  unitLabel: string;
  onShowTable?: () => void;
  /** Pixel width; measured from the container when omitted. */
  width?: number;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [measured, setMeasured] = useState(DEFAULT_WIDTH);
  const [active, setActive] = useState<number | null>(null);
  useEffect(() => {
    const el = box.current;
    if (!el || fixedWidth !== undefined || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      const w = Math.floor(el.getBoundingClientRect().width);
      if (w > 0) setMeasured(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [fixedWidth]);
  const width = fixedWidth ?? measured;
  const max = data.reduce((m, d) => Math.max(m, d.value), 0);
  const tk = ticks(max);
  const top = tk[tk.length - 1] ?? 1;
  const plotW = Math.max(1, width - PAD.left - PAD.right);
  const plotH = HEIGHT - PAD.top - PAD.bottom;
  const slot = data.length > 0 ? plotW / data.length : plotW;
  const y = (v: number) => PAD.top + plotH - (v / top) * plotH;
  const total = data.reduce((s, d) => s + d.value, 0);
  const summary = `${unitLabel}: ${data.length} ${data.length === 1 ? "bar" : "bars"}, total ${format(total)}, highest ${format(max)}`;
  const move = (e: KeyboardEvent) => {
    if (data.length === 0) return;
    const cur = active ?? -1;
    const next =
      e.key === "ArrowRight" ? Math.min(data.length - 1, cur + 1)
      : e.key === "ArrowLeft" ? Math.max(0, cur < 0 ? 0 : cur - 1)
      : e.key === "Home" ? 0
      : e.key === "End" ? data.length - 1
      : null;
    if (next === null) return;
    e.preventDefault();
    setActive(next);
  };
  const tip = active !== null ? data[active] : undefined;
  const tipText = tip ? `${tip.label}: ${format(tip.value)}` : "";
  const tipW = tipText.length * 6.6 + 16;
  const tipX = tip && active !== null ? Math.max(2, Math.min(width - tipW - 2, PAD.left + slot * active + slot / 2 - tipW / 2)) : 0;
  const tipY = tip ? Math.max(2, y(tip.value) - 32) : 0;
  const labelEvery = Math.max(1, Math.ceil(data.length / Math.max(1, Math.floor(plotW / 64))));
  return (
    <div className="space-y-2">
      <div
        ref={box}
        tabIndex={0}
        role="group"
        aria-roledescription="chart"
        aria-label={`${unitLabel}. Use the arrow keys to move between bars.`}
        onKeyDown={move}
        onFocus={() => setActive((a) => a ?? (data.length > 0 ? data.length - 1 : null))}
        onBlur={() => setActive(null)}
        onMouseLeave={() => setActive(null)}
        className="w-full overflow-hidden rounded-control focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
        data-testid="bar-chart"
      >
        <svg role="img" aria-label={summary} width={width} height={HEIGHT} viewBox={`0 0 ${width} ${HEIGHT}`} className="block max-w-full">
          {tk.map((t) => (
            <g key={t}>
              <line x1={PAD.left} x2={width - PAD.right} y1={y(t)} y2={y(t)} className="stroke-line" strokeWidth={1} />
              <text x={PAD.left - 6} y={y(t) + 4} textAnchor="end" className="fill-muted text-xs">
                {compact(t)}
              </text>
            </g>
          ))}
          {data.map((d, i) => {
            const h = Math.max(0, plotH - (y(d.value) - PAD.top));
            const x = PAD.left + slot * i + GAP / 2;
            const w = Math.max(1, slot - GAP);
            return (
              <g key={d.key} onMouseEnter={() => setActive(i)}>
                {/* a full-height hit area so a short bar is still hoverable */}
                <rect x={PAD.left + slot * i} y={PAD.top} width={slot} height={plotH} fill="transparent" data-bar={d.key} />
                {h > 0 ? <path d={barPath(x, y(d.value), w, h)} className={active === i ? "fill-accent-ink" : "fill-accent"} /> : null}
                {i % labelEvery === 0 ? (
                  <text x={x + w / 2} y={HEIGHT - 8} textAnchor="middle" className="fill-muted text-xs">
                    {d.label}
                  </text>
                ) : null}
              </g>
            );
          })}
          {tip ? (
            <g transform={`translate(${tipX} ${tipY})`} pointerEvents="none" data-testid="bar-tooltip">
              <rect width={tipW} height={24} rx={6} className="fill-ink" />
              <text x={8} y={16} className="fill-surface text-xs">
                {tipText}
              </text>
            </g>
          ) : null}
        </svg>
        {/* each bar's accessible name: "<label>: <value>" */}
        <ul className="sr-only" aria-label={`${unitLabel} by bar`}>
          {data.map((d) => (
            <li key={d.key}>{`${d.label}: ${format(d.value)}`}</li>
          ))}
        </ul>
        <span className="sr-only" aria-live="polite">
          {tipText}
        </span>
      </div>
      {onShowTable ? (
        <Button variant="ghost" size="sm" onClick={onShowTable}>
          Show as table
        </Button>
      ) : null}
    </div>
  );
}
