/**
 * (P5-REFINE-19, D-413) A restricted YAML: strings, numbers, booleans, null, arrays and maps, two-space block style. It is
 * the file format of an agent export (`agent.yaml`, `automations/*.yaml`), written by one function and read by one that
 * refuses everything it does not write: anchors (`&a`), aliases (`*a`), tags (`!x`, `!!str`), block scalars (`|`, `>`),
 * merge keys, directives and documents, flow collections other than `[]` and `{}`, tabs and duplicate keys.
 */

export type YamlValue = string | number | boolean | null | YamlValue[] | { [key: string]: YamlValue };

export class YamlError extends Error {}

const MAX_LINES = 5000;
const MAX_DEPTH = 24;

const PLAIN_KEY = /^[A-Za-z_][A-Za-z0-9_-]*$/;
// A plain scalar the reader would read as something other than the string: keep these quoted.
const RESERVED = /^(?:true|false|null|yes|no|on|off|y|n|~)$/i;
const NUMBERISH = /^[-+]?(?:\d[\d_]*\.?\d*(?:[eE][-+]?\d+)?|\.\d+|0x[0-9a-fA-F]+|0o[0-7]+|\.inf|\.nan)$/i;
const PLAIN_STRING = /^[A-Za-z][A-Za-z0-9 _./@-]*$/;

function scalar(v: string | number | boolean | null): string {
  if (v === null) return "null";
  if (typeof v === "boolean") return v ? "true" : "false";
  if (typeof v === "number") {
    if (!Number.isFinite(v)) throw new YamlError("Numbers must be finite.");
    return String(v);
  }
  // Plain only when nothing could be read differently; everything else is a JSON-escaped double-quoted string.
  if (PLAIN_STRING.test(v) && !RESERVED.test(v) && !NUMBERISH.test(v) && !v.endsWith(" ") && !v.includes("  ")) return v;
  return JSON.stringify(v);
}

function key(k: string): string {
  return PLAIN_KEY.test(k) && !RESERVED.test(k) ? k : JSON.stringify(k);
}

function emit(v: YamlValue, indent: number, out: string[], depth: number): void {
  if (depth > MAX_DEPTH) throw new YamlError("Too deeply nested.");
  const pad = " ".repeat(indent);
  if (Array.isArray(v)) {
    for (const item of v) {
      if (item !== null && typeof item === "object") {
        const empty = Array.isArray(item) ? item.length === 0 : Object.keys(item).length === 0;
        if (empty) out.push(`${pad}- ${Array.isArray(item) ? "[]" : "{}"}`);
        else if (Array.isArray(item)) {
          // A list in a list: a bare dash, the items on the lines below it.
          out.push(`${pad}-`);
          emit(item, indent + 2, out, depth + 1);
        } else {
          // `- ` then the first line of the nested block, the rest indented under it.
          const sub: string[] = [];
          emit(item, indent + 2, sub, depth + 1);
          out.push(`${pad}- ${sub[0]!.slice(indent + 2)}`, ...sub.slice(1));
        }
      } else out.push(`${pad}- ${scalar(item)}`);
    }
    return;
  }
  if (v !== null && typeof v === "object") {
    for (const [k, val] of Object.entries(v)) {
      if (val === undefined) continue;
      if (val !== null && typeof val === "object") {
        const empty = Array.isArray(val) ? val.length === 0 : Object.keys(val).length === 0;
        if (empty) out.push(`${pad}${key(k)}: ${Array.isArray(val) ? "[]" : "{}"}`);
        else {
          out.push(`${pad}${key(k)}:`);
          emit(val, indent + 2, out, depth + 1);
        }
      } else out.push(`${pad}${key(k)}: ${scalar(val)}`);
    }
    return;
  }
  out.push(`${pad}${scalar(v)}`);
}

/** The document for `value`: always ends with a newline. */
export function stringifyYaml(value: YamlValue): string {
  const out: string[] = [];
  if (value !== null && typeof value === "object" && !(Array.isArray(value) ? value.length : Object.keys(value).length)) {
    return `${Array.isArray(value) ? "[]" : "{}"}\n`;
  }
  emit(value, 0, out, 0);
  return `${out.join("\n")}\n`;
}

interface Line {
  indent: number;
  text: string;
  no: number;
}

function lines(src: string): Line[] {
  const raw = src.replace(/\r\n?/g, "\n").split("\n");
  if (raw.length > MAX_LINES) throw new YamlError(`Too many lines (the limit is ${MAX_LINES}).`);
  const out: Line[] = [];
  raw.forEach((l, i) => {
    if (l.includes("\t")) throw new YamlError(`Line ${i + 1}: tabs are not allowed.`);
    const trimmed = l.trimEnd();
    if (trimmed === "" || trimmed.trimStart().startsWith("#")) return;
    if (/^(---|\.\.\.|%)/.test(trimmed)) throw new YamlError(`Line ${i + 1}: documents and directives are not allowed.`);
    const indent = trimmed.length - trimmed.trimStart().length;
    out.push({ indent, text: trimmed.trimStart(), no: i + 1 });
  });
  return out;
}

function parseScalar(raw: string, no: number): YamlValue {
  const t = raw.trim();
  if (t === "") return null;
  const c = t[0]!;
  if (c === "&" || c === "*" || c === "!") throw new YamlError(`Line ${no}: anchors, aliases and tags are not allowed.`);
  if (c === "|" || c === ">") throw new YamlError(`Line ${no}: block scalars are not allowed.`);
  if (c === "\"") {
    // A JSON string, nothing after it (a trailing comment is not accepted either: the writer never writes one).
    let end = -1;
    for (let i = 1; i < t.length; i++) {
      if (t[i] === "\\") i++;
      else if (t[i] === "\"") {
        end = i;
        break;
      }
    }
    if (end !== t.length - 1) throw new YamlError(`Line ${no}: a quoted string must end the line.`);
    try {
      const v: unknown = JSON.parse(t);
      if (typeof v === "string") return v;
    } catch {
      // fall through
    }
    throw new YamlError(`Line ${no}: the quoted string has an escape this format does not read.`);
  }
  if (c === "'") throw new YamlError(`Line ${no}: single-quoted strings are not allowed; use double quotes.`);
  if (t === "[]") return [];
  if (t === "{}") return {};
  if (c === "[" || c === "{") throw new YamlError(`Line ${no}: flow collections are not allowed.`);
  if (t === "null" || t === "~") return null;
  if (t === "true") return true;
  if (t === "false") return false;
  if (/^[-+]?\d+$/.test(t)) {
    const n = Number(t);
    if (Number.isSafeInteger(n)) return n;
  }
  if (/^[-+]?\d+\.\d+$/.test(t)) return Number(t);
  if (t.includes(": ") || t.endsWith(":") || t.includes(" #")) throw new YamlError(`Line ${no}: quote the text “${t.slice(0, 40)}”.`);
  return t;
}

function splitKey(text: string, no: number): { k: string; rest: string } | null {
  if (text.startsWith("\"")) {
    let end = -1;
    for (let i = 1; i < text.length; i++) {
      if (text[i] === "\\") i++;
      else if (text[i] === "\"") {
        end = i;
        break;
      }
    }
    if (end < 0 || text[end + 1] !== ":" || (text[end + 2] !== undefined && text[end + 2] !== " ")) return null;
    try {
      return { k: JSON.parse(text.slice(0, end + 1)) as string, rest: text.slice(end + 2).trim() };
    } catch {
      throw new YamlError(`Line ${no}: the quoted key is not valid.`);
    }
  }
  const m = /^([A-Za-z_][A-Za-z0-9_-]*):(?: (.*))?$/.exec(text);
  return m ? { k: m[1]!, rest: (m[2] ?? "").trim() } : null;
}

/** Reads the restricted subset; throws `YamlError` (one sentence, with the line) for anything else. */
export function parseYaml(src: string): YamlValue {
  const ls = lines(src);
  if (ls.length === 0) return null;
  let at = 0;

  function block(indent: number, depth: number): YamlValue {
    if (depth > MAX_DEPTH) throw new YamlError("Too deeply nested.");
    const first = ls[at]!;
    if (first.indent !== indent) throw new YamlError(`Line ${first.no}: unexpected indentation.`);
    return first.text === "-" || first.text.startsWith("- ") ? seq(indent, depth) : map(indent, depth);
  }

  function seq(indent: number, depth: number): YamlValue[] {
    const out: YamlValue[] = [];
    while (at < ls.length && ls[at]!.indent === indent && (ls[at]!.text === "-" || ls[at]!.text.startsWith("- "))) {
      const l = ls[at]!;
      const rest = l.text === "-" ? "" : l.text.slice(2).trimStart();
      if (rest === "") {
        at++;
        const next = ls[at];
        out.push(next && next.indent > indent ? block(next.indent, depth + 1) : null);
      } else if (splitKey(rest, l.no) !== null) {
        // `- key: value`: the item is a map whose keys sit two columns in from the dash.
        ls[at] = { indent: indent + 2, text: rest, no: l.no };
        out.push(map(indent + 2, depth + 1));
      } else {
        at++;
        out.push(parseScalar(rest, l.no));
      }
    }
    return out;
  }

  function map(indent: number, depth: number): { [k: string]: YamlValue } {
    const out: { [k: string]: YamlValue } = Object.create(null) as { [k: string]: YamlValue };
    while (at < ls.length && ls[at]!.indent === indent) {
      const l = ls[at]!;
      if (l.text === "-" || l.text.startsWith("- ")) throw new YamlError(`Line ${l.no}: a list item where a key was expected.`);
      const kv = splitKey(l.text, l.no);
      if (!kv) throw new YamlError(`Line ${l.no}: expected “key: value”.`);
      if (kv.k === "<<") throw new YamlError(`Line ${l.no}: merge keys are not allowed.`);
      if (kv.k in out) throw new YamlError(`Line ${l.no}: the key “${kv.k}” appears twice.`);
      at++;
      if (kv.rest === "") {
        const next = ls[at];
        // A list may sit at the same column as its key (`key:` then `- x`), as the writer never does but YAML allows.
        if (next && next.indent > indent) out[kv.k] = block(next.indent, depth + 1);
        else if (next && next.indent === indent && (next.text === "-" || next.text.startsWith("- "))) out[kv.k] = seq(indent, depth + 1);
        else out[kv.k] = null;
      } else out[kv.k] = parseScalar(kv.rest, l.no);
    }
    if (at < ls.length && ls[at]!.indent > indent) throw new YamlError(`Line ${ls[at]!.no}: unexpected indentation.`);
    return out;
  }

  let value: YamlValue;
  if (ls.length === 1 && splitKey(ls[0]!.text, ls[0]!.no) === null && !ls[0]!.text.startsWith("-")) {
    // A document that is one scalar.
    value = parseScalar(ls[0]!.text, ls[0]!.no);
    at = 1;
  } else value = block(ls[0]!.indent, 0);
  if (at < ls.length) throw new YamlError(`Line ${ls[at]!.no}: unexpected indentation.`);
  return plain(value);
}

/** Maps made with a null prototype become ordinary objects. */
function plain(v: YamlValue): YamlValue {
  if (Array.isArray(v)) return v.map(plain);
  if (v !== null && typeof v === "object") return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, plain(x)]));
  return v;
}
