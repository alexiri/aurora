/**
 * The field-by-field difference between two configuration fingerprints (C-866, C-883; D-291): exactly the fields that
 * changed, in words. The diff is the attribution; the page never guesses a cause. A value that is a hash is shown
 * truncated by the caller (`isHash`).
 */

export interface FieldChange {
  /** Stable key, for React lists and tests. */
  key: string;
  /** The field in words. */
  field: string;
  before: string;
  after: string;
  /** A hash or an id: the page truncates it and offers a copy action. */
  hash: boolean;
}

const NONE = "not set";

const LABEL: Record<string, string> = {
  instructions_sha256: "Instructions (hash)",
  harness: "Harness",
  "model.provider": "Model provider",
  "model.endpoint_id": "Model endpoint",
  "model.model_id": "Model",
  effort: "Effort",
  "model_roles.large": "Large model",
  "model_roles.medium": "Medium model",
  "model_roles.small": "Small model",
  subagent_model: "Subagent model",
  "judge.secret_id": "Judge key",
  "judge.model_id": "Judge model",
  question_set_version: "Question set (version hash)",
};

const HASHY = new Set(["instructions_sha256", "question_set_version", "judge.secret_id"]);

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);

const show = (v: unknown): string => (v === null || v === undefined || v === "" ? NONE : String(v));

/** Flattens the object to `path -> value`; the skill list and the granted rule ids become one entry per item. */
function flatten(fp: unknown): Map<string, string> {
  const out = new Map<string, string>();
  if (!isObj(fp)) return out;
  const walk = (prefix: string, v: unknown) => {
    for (const [k, val] of Object.entries(v as Obj)) {
      const path = prefix ? `${prefix}.${k}` : k;
      if (path === "skills" && Array.isArray(val)) {
        for (const s of val) {
          if (isObj(s)) out.set(`skill:${String(s.tier)}/${String(s.name)}`, show(s.content_sha256));
        }
      } else if (path === "granted_rule_ids" && Array.isArray(val)) {
        for (const id of val) out.set(`rule:${String(id)}`, "granted");
      } else if (isObj(val)) walk(path, val);
      else out.set(path, show(val));
    }
  };
  walk("", fp);
  return out;
}

const fieldLabel = (key: string): string => {
  if (key.startsWith("skill:")) return `Skill ${key.slice(6)}`;
  if (key.startsWith("rule:")) return `Granted rule ${key.slice(5)}`;
  return LABEL[key] ?? key;
};

const ORDER = Object.keys(LABEL);
const rank = (key: string) => {
  const i = ORDER.indexOf(key);
  if (i >= 0) return i;
  return key.startsWith("skill:") ? ORDER.length : key.startsWith("rule:") ? ORDER.length + 1 : ORDER.length + 2;
};

/** The changed fields between `before` and `after` (either may be absent: an older run without a field). */
export function diffFingerprints(before: unknown, after: unknown): FieldChange[] {
  const a = flatten(before);
  const b = flatten(after);
  const keys = [...new Set([...a.keys(), ...b.keys()])];
  const changes: FieldChange[] = [];
  for (const key of keys) {
    // A field one run did not record at all reads as "not set", the same as a null (an older run, C-883).
    const item = key.startsWith("skill:") || key.startsWith("rule:");
    const x = a.get(key) ?? (item ? undefined : NONE);
    const y = b.get(key) ?? (item ? undefined : NONE);
    if (x === y) continue;
    const absent = key.startsWith("skill:") ? "not installed" : key.startsWith("rule:") ? "not granted" : NONE;
    changes.push({
      key,
      field: fieldLabel(key),
      before: x ?? absent,
      after: y ?? absent,
      hash: HASHY.has(key) || key.startsWith("skill:") || key.startsWith("rule:"),
    });
  }
  return changes.sort((p, q) => rank(p.key) - rank(q.key) || p.key.localeCompare(q.key));
}

/** A hash or id cut to its first characters for the table; the copy action carries the whole value. */
export const truncateHash = (s: string, n = 12): string => (s.length > n + 1 ? `${s.slice(0, n)}…` : s);
