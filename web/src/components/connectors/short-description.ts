/** First line of a setup guide as plain text, for the type card. */
export function shortDescription(md: string | null | undefined): string {
  const line = (md ?? "").split("\n").map((l) => l.trim()).find((l) => l && !l.startsWith("#")) ?? "";
  const text = line.replace(/\[([^\]]*)\]\([^)]*\)/g, "$1").replace(/[*_`>]/g, "").trim();
  return text.length > 160 ? `${text.slice(0, 157)}…` : text;
}
