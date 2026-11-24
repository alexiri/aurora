/**
 * (P5-REFINE-19, D-413) A small zip writer and reader for the agent export: no dependency. The writer stores (method 0); the
 * reader also inflates (method 8, through the browser's `DecompressionStream`) so a zip somebody re-packed still opens.
 * The reader is the part that meets untrusted bytes: it refuses encrypted and Zip64 archives, more than `MAX_FILES` files,
 * more than `MAX_BYTES` once unpacked, and any name that is absolute, hidden or climbs out (`..`).
 */

export const MAX_ZIP_BYTES = 5 * 1024 * 1024;
export const MAX_FILES = 200;
export const MAX_UNPACKED_BYTES = 5 * 1024 * 1024;

export interface ZipEntry {
  name: string;
  data: Uint8Array;
  /**
   * (P6-TPL-5, C-1141) The Unix executable bit, needed for a template's `helpers/` files. The agent
   * export never sets it, so its bytes stay exactly what they were.
   */
  executable?: boolean;
}

/** An error whose message is one sentence for the person. */
export class ZipError extends Error {}

const enc = new TextEncoder();
const dec = new TextDecoder("utf-8", { fatal: true });

let table: Uint32Array | null = null;
function crcTable(): Uint32Array {
  if (table) return table;
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  table = t;
  return t;
}

export function crc32(data: Uint8Array): number {
  const t = crcTable();
  let c = 0xffffffff;
  for (let i = 0; i < data.length; i++) c = t[(c ^ data[i]!) & 0xff]! ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/** Why a name may not be unpacked, or null. Names are `/`-separated, relative, with no hidden or `..` segment. */
export function nameProblem(name: string): string | null {
  if (name === "" || name.length > 255) return "a file name is empty or too long";
  if (name.includes("\0") || name.includes("\\")) return `the file name “${printable(name)}” has characters that are not allowed`;
  if (name.startsWith("/") || /^[a-zA-Z]:/.test(name)) return `the file name “${printable(name)}” is an absolute path`;
  for (const seg of name.split("/")) {
    if (seg === "..") return `the file name “${printable(name)}” climbs out of the folder`;
    if (seg.startsWith(".")) return `the file name “${printable(name)}” is a hidden file`;
  }
  return null;
}

function printable(s: string): string {
  return s.replace(/[^\x20-\x7e]/g, "?").slice(0, 80);
}

function dosDateTime(d: Date): { time: number; date: number } {
  const y = Math.max(1980, d.getFullYear());
  return { time: (d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1), date: ((y - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate() };
}

/** A store-only zip of `entries`, in order. Names are written as UTF-8. */
export function writeZip(entries: ZipEntry[], now: Date = new Date()): Uint8Array {
  const { time, date } = dosDateTime(now);
  const chunks: Uint8Array[] = [];
  const central: Uint8Array[] = [];
  let offset = 0;
  for (const e of entries) {
    const problem = nameProblem(e.name);
    if (problem) throw new ZipError(`Cannot write the zip: ${problem}.`);
    const name = enc.encode(e.name);
    const crc = crc32(e.data);
    const local = new Uint8Array(30 + name.length);
    const lv = new DataView(local.buffer);
    lv.setUint32(0, 0x04034b50, true);
    lv.setUint16(4, 20, true);
    lv.setUint16(6, 0x0800, true);
    lv.setUint16(8, 0, true);
    lv.setUint16(10, time, true);
    lv.setUint16(12, date, true);
    lv.setUint32(14, crc, true);
    lv.setUint32(18, e.data.length, true);
    lv.setUint32(22, e.data.length, true);
    lv.setUint16(26, name.length, true);
    lv.setUint16(28, 0, true);
    local.set(name, 30);
    const cd = new Uint8Array(46 + name.length);
    const cv = new DataView(cd.buffer);
    cv.setUint32(0, 0x02014b50, true);
    cv.setUint16(4, 20, true);
    cv.setUint16(6, 20, true);
    cv.setUint16(8, 0x0800, true);
    cv.setUint16(10, 0, true);
    cv.setUint16(12, time, true);
    cv.setUint16(14, date, true);
    cv.setUint32(16, crc, true);
    cv.setUint32(20, e.data.length, true);
    cv.setUint32(24, e.data.length, true);
    cv.setUint16(28, name.length, true);
    // (P6-TPL-5) The executable bit rides in the high bytes of the external attributes, as the
    // readers of both sides (web and Rust) expect; zero otherwise, so exports are unchanged.
    if (e.executable) cv.setUint32(38, (0o100755 << 16) >>> 0, true);
    cv.setUint32(42, offset, true);
    cd.set(name, 46);
    chunks.push(local, e.data);
    central.push(cd);
    offset += local.length + e.data.length;
  }
  const cdSize = central.reduce((n, c) => n + c.length, 0);
  const end = new Uint8Array(22);
  const ev = new DataView(end.buffer);
  ev.setUint32(0, 0x06054b50, true);
  ev.setUint16(8, entries.length, true);
  ev.setUint16(10, entries.length, true);
  ev.setUint32(12, cdSize, true);
  ev.setUint32(16, offset, true);
  const out = new Uint8Array(offset + cdSize + 22);
  let at = 0;
  for (const c of [...chunks, ...central, end]) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

async function inflateRaw(data: Uint8Array, limit: number): Promise<Uint8Array> {
  const Ds = (globalThis as { DecompressionStream?: new (f: string) => GenericTransformStream }).DecompressionStream;
  if (!Ds) throw new ZipError("This browser cannot open compressed zips. Zip the folder again without compression.");
  const stream = new Blob([data as BlobPart]).stream().pipeThrough(new Ds("deflate-raw"));
  const reader = stream.getReader();
  const parts: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.length;
    if (total > limit) {
      void reader.cancel();
      throw new ZipError("The zip is too large once unpacked (the limit is 5 MB).");
    }
    parts.push(value);
  }
  const out = new Uint8Array(total);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

/** The files of a zip, folders left out. Refuses anything it cannot read safely, with one sentence naming the problem. */
export async function readZip(bytes: Uint8Array): Promise<ZipEntry[]> {
  if (bytes.length > MAX_ZIP_BYTES) throw new ZipError("The zip is larger than 5 MB.");
  if (bytes.length < 22) throw new ZipError("This is not a zip file.");
  const v = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  // The end-of-central-directory record: the last 22..(22 + 65535) bytes.
  let eocd = -1;
  for (let i = bytes.length - 22; i >= Math.max(0, bytes.length - 22 - 0xffff); i--) {
    if (v.getUint32(i, true) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw new ZipError("This is not a zip file.");
  const count = v.getUint16(eocd + 10, true);
  const cdSize = v.getUint32(eocd + 12, true);
  const cdOffset = v.getUint32(eocd + 16, true);
  if (count === 0xffff || cdSize === 0xffffffff || cdOffset === 0xffffffff) throw new ZipError("Zip64 archives are not supported.");
  if (count > MAX_FILES + 400) throw new ZipError(`The zip has more than ${MAX_FILES} files.`);
  const out: ZipEntry[] = [];
  let pos = cdOffset;
  let unpacked = 0;
  for (let i = 0; i < count; i++) {
    if (pos + 46 > bytes.length || v.getUint32(pos, true) !== 0x02014b50) throw new ZipError("The zip is damaged.");
    const flags = v.getUint16(pos + 8, true);
    const method = v.getUint16(pos + 10, true);
    const crc = v.getUint32(pos + 16, true);
    const csize = v.getUint32(pos + 20, true);
    const usize = v.getUint32(pos + 24, true);
    const nlen = v.getUint16(pos + 28, true);
    const xlen = v.getUint16(pos + 30, true);
    const clen = v.getUint16(pos + 32, true);
    const local = v.getUint32(pos + 42, true);
    if (pos + 46 + nlen > bytes.length) throw new ZipError("The zip is damaged.");
    let name: string;
    try {
      name = dec.decode(bytes.subarray(pos + 46, pos + 46 + nlen));
    } catch {
      throw new ZipError("A file name in the zip is not valid text.");
    }
    pos += 46 + nlen + xlen + clen;
    if (name.endsWith("/")) continue;
    if (flags & 1) throw new ZipError("The zip is encrypted.");
    const problem = nameProblem(name);
    if (problem) throw new ZipError(`Refused: ${problem}.`);
    if (out.length >= MAX_FILES) throw new ZipError(`The zip has more than ${MAX_FILES} files.`);
    unpacked += usize;
    if (unpacked > MAX_UNPACKED_BYTES) throw new ZipError("The zip is too large once unpacked (the limit is 5 MB).");
    if (csize === 0xffffffff || usize === 0xffffffff) throw new ZipError("Zip64 archives are not supported.");
    if (local + 30 > bytes.length || v.getUint32(local, true) !== 0x04034b50) throw new ZipError("The zip is damaged.");
    const start = local + 30 + v.getUint16(local + 26, true) + v.getUint16(local + 28, true);
    if (start + csize > bytes.length) throw new ZipError("The zip is damaged.");
    const raw = bytes.subarray(start, start + csize);
    let data: Uint8Array;
    if (method === 0) data = raw.slice();
    else if (method === 8) data = await inflateRaw(raw, MAX_UNPACKED_BYTES - (unpacked - usize));
    else throw new ZipError("The zip uses a compression this page cannot read.");
    if (data.length !== usize || crc32(data) !== crc) throw new ZipError(`“${printable(name)}” is damaged in the zip.`);
    out.push({ name, data });
  }
  return out;
}
