import { cn } from "@/lib/utils";

const TINTS = [
  "bg-avatar-lavender text-avatar-lavender-ink",
  "bg-avatar-blue text-avatar-blue-ink",
  "bg-accent-soft text-accent-ink",
];

// Fixed sizes use classes (the production CSP blocks inline style attributes).
const SIZES: Record<number, string> = {
  20: "size-5 text-micro",
  24: "size-6 text-label",
  28: "size-7 text-xs",
  32: "size-8 text-sm",
  36: "size-9 text-base",
  40: "size-10 text-lg",
  44: "size-11 text-lg",
  48: "size-12 text-xl",
  56: "size-14 text-2xl",
  64: "size-16 text-3xl",
  72: "size-18 text-3xl",
};

/** Stable hash so one agent keeps one tint. (Also used by the sidebar selects' initial chip.) */
export function tintFor(key: string) {
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
  return TINTS[h % TINTS.length];
}

/** Avatar: the public image when set (a same-origin path), otherwise an initial on a tinted tile. */
export function Avatar({
  name,
  url,
  size = 32,
  tintKey,
  className,
}: {
  name: string;
  url?: string | null;
  size?: number;
  /** Stable id (e.g. the agent id) the tint is chosen from; defaults to the name. */
  tintKey?: string;
  className?: string;
}) {
  const sizeClass = SIZES[size] ?? SIZES[32];
  if (url) {
    // eslint-disable-next-line @next/next/no-img-element
    return <img src={url} alt="" width={size} height={size} className={cn("shrink-0 rounded-full object-cover", sizeClass, className)} />;
  }
  return (
    <span
      aria-hidden="true"
      className={cn(
        "inline-flex shrink-0 items-center justify-center rounded-full font-semibold uppercase",
        sizeClass,
        tintFor(tintKey ?? name),
        className,
      )}
    >
      {name.trim().charAt(0) || "?"}
    </span>
  );
}

/** Agent avatar (kept for existing call sites). */
export const AgentAvatar = Avatar;
