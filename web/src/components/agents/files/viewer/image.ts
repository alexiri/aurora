// Image preview helpers (P5-FILES-3, C-905): the type of a file is decided by its first bytes, never by its name, and
// only four raster formats are ever shown. SVG (script-capable markup) and everything else get the "No preview" card.

export type ImageMime = "image/png" | "image/jpeg" | "image/gif" | "image/webp";

const ascii = (b: Uint8Array, at: number, text: string) => text.split("").every((c, i) => b[at + i] === c.charCodeAt(0));

/** The raster image type of `bytes` by magic number (PNG, JPEG, GIF87a/89a, WebP), or null. */
export function sniffImage(bytes: Uint8Array): ImageMime | null {
  if (bytes.length >= 8 && [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a].every((v, i) => bytes[i] === v)) return "image/png";
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) return "image/jpeg";
  if (bytes.length >= 6 && (ascii(bytes, 0, "GIF87a") || ascii(bytes, 0, "GIF89a"))) return "image/gif";
  if (bytes.length >= 12 && ascii(bytes, 0, "RIFF") && ascii(bytes, 8, "WEBP")) return "image/webp";
  return null;
}

/** A `data:` URL with the fixed MIME from the sniffed type (`img-src` allows `data:`; no blob URL, no new tab). */
export function imageDataUrl(bytes: Uint8Array, mime: ImageMime): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return `data:${mime};base64,${btoa(binary)}`;
}

const GUESS: Record<string, string> = {
  svg: "SVG image (not previewed: it can contain scripts)",
  png: "PNG image",
  jpg: "JPEG image",
  jpeg: "JPEG image",
  gif: "GIF image",
  webp: "WebP image",
  pdf: "PDF document",
  zip: "ZIP archive",
  gz: "Compressed archive",
  tar: "Archive",
  so: "Shared library",
  bin: "Binary file",
  exe: "Program",
  sqlite: "SQLite database",
  db: "Database",
};

/** A short description of what a file probably is, from its name (display only: nothing depends on it). */
export function typeGuess(name: string): string {
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
  return GUESS[ext] ?? (ext ? `.${ext} file` : "File");
}
