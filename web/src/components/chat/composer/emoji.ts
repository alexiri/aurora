/**
 * Emoji data for the composer (P5-CHAT-3): `node-emoji` 2.2.0 (MIT, ≈ 50 KB of data, the same package `remark-emoji`
 * already ships for the renderer), loaded on first use so it is not in the chat's first load.
 */
export interface EmojiHit {
  emoji: string;
  /** The shortcode without colons, e.g. `tada`. */
  name: string;
}

type Mod = typeof import("node-emoji");
let loaded: Promise<Mod> | null = null;
export function loadEmoji(): Promise<Mod> {
  return (loaded ??= import("node-emoji"));
}

/** Up to `limit` emoji whose shortcode starts with `q`, then those that contain it. */
export function rankEmoji(all: EmojiHit[], q: string, limit = 8): EmojiHit[] {
  const starts: EmojiHit[] = [];
  const contains: EmojiHit[] = [];
  for (const h of all) {
    if (h.name.startsWith(q)) starts.push(h);
    else if (h.name.includes(q)) contains.push(h);
  }
  const by = (a: EmojiHit, b: EmojiHit) => a.name.length - b.name.length || a.name.localeCompare(b.name);
  return [...starts.sort(by), ...contains.sort(by)].slice(0, limit);
}

export async function searchEmoji(q: string, limit = 8): Promise<EmojiHit[]> {
  const m = await loadEmoji();
  return rankEmoji(
    m.search(q).map((h) => ({ emoji: h.emoji, name: h.name })),
    q.toLowerCase(),
    limit,
  );
}

/** The picker's default grid: common shortcodes, in a fixed order. */
export const COMMON = [
  "+1",
  "-1",
  "heart",
  "tada",
  "rocket",
  "eyes",
  "pray",
  "clap",
  "fire",
  "100",
  "white_check_mark",
  "x",
  "warning",
  "bulb",
  "memo",
  "thinking",
  "smile",
  "grin",
  "joy",
  "sweat_smile",
  "wink",
  "slightly_smiling_face",
  "sob",
  "confused",
  "raised_hands",
  "muscle",
  "wave",
  "ok_hand",
  "bug",
  "sparkles",
  "zap",
  "hourglass",
];

export async function commonEmoji(): Promise<EmojiHit[]> {
  const m = await loadEmoji();
  const out: EmojiHit[] = [];
  for (const name of COMMON) {
    const emoji = m.get(name);
    if (emoji) out.push({ emoji, name });
  }
  return out;
}
