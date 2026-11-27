import { LEVEL_LABEL } from "@/lib/accounts";
import type { Level } from "@/lib/api/accounts";
import type {
  TemplateAppEntryView,
  TemplateAutomationView,
  TemplateContentView,
  TemplateInputView,
  TemplateSkillRefView,
} from "@/lib/api/agent-templates";

/**
 * The diff of two template versions (FIX-247, amending C-1114): a pure function that diffs two
 * `TemplateContentView`s — the version people use now against the one an administrator is reviewing —
 * into a typed list of changes covering every part, plus the summary in words, the changes that widen
 * what the agent can do or reach, and the parts nobody touched. Everything is data; rendering is the
 * review page's.
 */

export type ChangeKind = "added" | "removed" | "changed";

export interface TemplateChange {
  /** The part's stable machine key (`skill:code-review:body`); tests and keys use it. */
  part: string;
  /** Which part group the change belongs to (the summary and the unchanged list are per group). */
  group: string;
  /** The part's label, in the product's words. */
  label: string;
  kind: ChangeKind;
  /** The change widens what the agent can do or reach — an administrator decides on these first. */
  attention: boolean;
  /** For text parts: the old and new text, in full. */
  old?: string;
  new?: string;
  /** For parts that are not (only) text: the change in words. */
  detail?: string;
  /** For a whole skill added or removed (`old`/`new` are its body): its **Use it when** text. */
  useItWhen?: string;
}

export interface TemplateDiff {
  /** Every change, in the order of the review page's parts. */
  changes: TemplateChange[];
  /** The one-line summary in words ("5 changes: Instructions, 2 skills, a new app, a new schedule"). */
  summary: string;
  /** The changes to look at first: the ones that widen what the agent can do or reach. */
  attention: TemplateChange[];
  /** The part groups with no change, as "Unchanged:" labels with their counts, in page order. */
  unchanged: string[];
}

/** The name of an app in the catalogue; the identity keeps unknown apps readable. */
export type AppNamer = (appId: string) => string;

const LEVEL_RANK: Record<string, number> = { read: 0, ask: 1, act: 2 };

const VERIFICATION_WORDS: Record<string, string> = {
  signed: "verified with a signature",
  password: "verified with a password",
  none: "not verified",
};

/** A text part's change: added when only the new version has it, removed when only the old one does. */
function textChange(part: string, group: string, label: string, oldText: string | null | undefined, newText: string | null | undefined, attention = false): TemplateChange | null {
  if (oldText === newText) return null;
  const kind: ChangeKind = oldText == null ? "added" : newText == null ? "removed" : "changed";
  return { part, group, label, kind, attention, old: oldText ?? undefined, new: newText ?? undefined };
}

type DiffFile = { text?: string | null; sha256: string; executable: boolean };

/**
 * One file's change: its content, and (FIX-275) its executable bit, which is a change by itself — a file that becomes
 * runnable is never left as unchanged. The bit is said in `detail`, beside the content diff when there is one.
 */
function fileChange(part: string, group: string, label: string, b: DiffFile | undefined, n: DiffFile | undefined, attention = false): TemplateChange | null {
  const content = contentChange(part, group, label, b, n, attention);
  if (!b || !n || b.executable === n.executable) return content;
  const mode = n.executable ? "This file is now runnable." : "This file is no longer runnable.";
  if (!content) return { part, group, label, kind: "changed", attention, detail: mode };
  return { ...content, detail: content.detail ? `${content.detail} ${mode}` : mode };
}

/**
 * A file's content change, by presence: a file that is not text is still added, removed or changed (compared by its
 * hash), and one that stops or starts being text is changed, not removed or added. With a side that is not text there is
 * no line comparison, so the change carries words and neither text.
 */
function contentChange(part: string, group: string, label: string, b: DiffFile | undefined, n: DiffFile | undefined, attention: boolean): TemplateChange | null {
  const oldText = b?.text ?? null;
  const newText = n?.text ?? null;
  if ((!b || oldText != null) && (!n || newText != null)) return textChange(part, group, label, b ? oldText : null, n ? newText : null, attention);
  if (b && n && oldText === newText && b.sha256 === n.sha256) return null;
  const kind: ChangeKind = !b ? "added" : !n ? "removed" : "changed";
  const what = kind === "added" ? "New in this version." : kind === "removed" ? "Not in this version any more." : "It changed.";
  const why = b && n && newText != null ? "It was not text before" : "It is not text";
  return { part, group, label, kind, attention, detail: `${what} ${why}, so there is no line comparison.` };
}

/** The access one app entry asks for, in words (the review page's wording). */
function appAccessWords(a: TemplateAppEntryView): string {
  return a.tools
    ? `custom access: may use ${(a.tools.allow ?? []).join(", ") || "nothing"}${(a.tools.ask ?? []).length ? `; asks before ${(a.tools.ask ?? []).join(", ")}` : ""}`
    : `${LEVEL_LABEL[a.level as Level] ?? a.level} access`;
}

function appEntryWords(a: TemplateAppEntryView, appName: AppNamer): string {
  const display = a.app ? appName(a.app) : (a.any_of ?? []).map(appName).join(" or ");
  return `${display} (${a.required ? "required" : "optional"}, ${appAccessWords(a)})`;
}

/** Whether one app entry asks for more than the other: a higher access level, or more tools it may use. */
function appWidened(b: TemplateAppEntryView, n: TemplateAppEntryView): boolean {
  if ((LEVEL_RANK[n.level] ?? 0) > (LEVEL_RANK[b.level] ?? 0)) return true;
  if (n.tools && b.tools) {
    const allow = new Set(b.tools.allow ?? []);
    return (n.tools.allow ?? []).some((t) => !allow.has(t));
  }
  return !!n.tools && !b.tools;
}

function diffApps(base: TemplateContentView, next: TemplateContentView, appName: AppNamer, out: TemplateChange[]) {
  const keyOf = (a: TemplateAppEntryView) => a.app ?? (a.any_of ?? []).join("|");
  const entries = new Map<string, { b?: TemplateAppEntryView; n?: TemplateAppEntryView }>();
  for (const a of base.apps as TemplateAppEntryView[]) entries.set(keyOf(a), { b: a });
  for (const a of next.apps as TemplateAppEntryView[]) {
    const e = entries.get(keyOf(a)) ?? {};
    e.n = a;
    entries.set(keyOf(a), e);
  }
  for (const [key, e] of entries) {
    const a = (e.n ?? e.b)!;
    const display = a.app ? appName(a.app) : (a.any_of ?? []).map(appName).join(" or ");
    const label = `App ${display}`;
    if (!e.b) {
      out.push({ part: `app:${key}`, group: "apps", label, kind: "added", attention: true, detail: `New in this version: ${appEntryWords(e.n!, appName)}. ${e.n!.why}` });
      continue;
    }
    if (!e.n) {
      out.push({ part: `app:${key}`, group: "apps", label, kind: "removed", attention: false, detail: `Not in this version any more: ${appEntryWords(e.b, appName)}.` });
      continue;
    }
    const b = e.b;
    const n = e.n;
    const details: string[] = [];
    if (b.required !== n.required) details.push(`It is now ${n.required ? "required" : "optional"}.`);
    if (b.level !== n.level) details.push(`Access changes from ${LEVEL_LABEL[b.level as Level] ?? b.level} to ${LEVEL_LABEL[n.level as Level] ?? n.level}.`);
    if (JSON.stringify(b.tools ?? null) !== JSON.stringify(n.tools ?? null)) details.push(`The tools it may use changed: now ${appAccessWords(n)}.`);
    if (b.why !== n.why) details.push("The reason it gives changed.");
    if (details.length === 0) continue;
    out.push({ part: `app:${key}`, group: "apps", label, kind: "changed", attention: appWidened(b, n), detail: details.join(" ") });
  }
}

function diffAutomations(base: TemplateContentView, next: TemplateContentView, out: TemplateChange[]) {
  const keyOf = (a: TemplateAutomationView) => `${a.kind}:${a.name}`;
  const entries = new Map<string, { b?: TemplateAutomationView; n?: TemplateAutomationView }>();
  for (const a of base.automations as TemplateAutomationView[]) entries.set(keyOf(a), { b: a });
  for (const a of next.automations as TemplateAutomationView[]) {
    const e = entries.get(keyOf(a)) ?? {};
    e.n = a;
    entries.set(keyOf(a), e);
  }
  for (const [key, e] of entries) {
    const a = (e.n ?? e.b)!;
    const label = `${a.kind === "webhook" ? "Webhook" : "Schedule"} ${a.name}`;
    if (!e.b) {
      out.push({ part: `automation:${key}`, group: "automations", label, kind: "added", attention: true, detail: a.kind === "schedule" ? `New in this version: it runs on its own (${a.prompt ?? ""}).` : "New in this version: callers can wake it." });
      continue;
    }
    if (!e.n) {
      out.push({ part: `automation:${key}`, group: "automations", label, kind: "removed", attention: false, detail: "Not in this version any more." });
      continue;
    }
    const b = e.b;
    const n = e.n;
    const details: string[] = [];
    if (n.kind === "schedule") {
      if (b.prompt !== n.prompt) details.push("What it does changed.");
      if (b.cron !== n.cron) details.push("When it runs changed.");
    } else {
      if (b.instructions !== n.instructions) details.push("What it is asked to do changed.");
      if (b.verification !== n.verification) details.push(`How it is verified changed: now ${VERIFICATION_WORDS[n.verification ?? "none"] ?? n.verification}.`);
    }
    if (details.length === 0) continue;
    out.push({ part: `automation:${key}`, group: "automations", label, kind: "changed", attention: false, detail: details.join(" ") });
  }
}

function diffSkillRefs(base: TemplateContentView, next: TemplateContentView, out: TemplateChange[]) {
  const keyOf = (r: TemplateSkillRefView) => (r.kind === "library" ? `library:${r.skill}` : `organization:${r.name}`);
  const entries = new Map<string, { b?: TemplateSkillRefView; n?: TemplateSkillRefView }>();
  for (const r of base.skill_refs as TemplateSkillRefView[]) entries.set(keyOf(r), { b: r });
  for (const r of next.skill_refs as TemplateSkillRefView[]) {
    const e = entries.get(keyOf(r)) ?? {};
    e.n = r;
    entries.set(keyOf(r), e);
  }
  for (const [key, e] of entries) {
    const r = (e.n ?? e.b)!;
    const label = r.kind === "library" ? `Library skill ${r.skill}` : `Organization skill ${r.name}`;
    if (!e.b) {
      out.push({ part: `skill_ref:${key}`, group: "skill_refs", label, kind: "added", attention: true, detail: `New in this version: the agent reads it from ${r.kind === "library" ? "a skill library" : "the organization"}. ${r.why}` });
      continue;
    }
    if (!e.n) {
      out.push({ part: `skill_ref:${key}`, group: "skill_refs", label, kind: "removed", attention: false, detail: "Not in this version any more." });
      continue;
    }
    const b = e.b;
    const n = e.n;
    const details: string[] = [];
    if (b.required !== n.required) details.push(`It is now ${n.required ? "required" : "optional"}.`);
    if (b.address !== n.address || b.ref !== n.ref || b.path !== n.path) details.push("Where it comes from changed.");
    if (b.why !== n.why) details.push("The reason it gives changed.");
    if (details.length === 0) continue;
    out.push({ part: `skill_ref:${key}`, group: "skill_refs", label, kind: "changed", attention: false, detail: details.join(" ") });
  }
}

function diffInputs(base: TemplateContentView, next: TemplateContentView, out: TemplateChange[]) {
  const keyOf = (i: TemplateInputView) => i.id;
  const entries = new Map<string, { b?: TemplateInputView; n?: TemplateInputView }>();
  for (const i of base.inputs as TemplateInputView[]) entries.set(keyOf(i), { b: i });
  for (const i of next.inputs as TemplateInputView[]) {
    const e = entries.get(keyOf(i)) ?? {};
    e.n = i;
    entries.set(keyOf(i), e);
  }
  for (const [key, e] of entries) {
    const i = (e.n ?? e.b)!;
    const label = `Input ${i.label || key}`;
    if (!e.b) {
      out.push({ part: `input:${key}`, group: "inputs", label, kind: "added", attention: true, detail: "New in this version: the person creating an agent is asked for it." });
      continue;
    }
    if (!e.n) {
      out.push({ part: `input:${key}`, group: "inputs", label, kind: "removed", attention: true, detail: "Not asked any more." });
      continue;
    }
    const b = e.b;
    const n = e.n;
    const details: string[] = [];
    if (b.label !== n.label) details.push("What it is called changed.");
    if (b.kind !== n.kind) details.push(n.kind === "choice" ? "It is now a choice." : "It is now a line of text.");
    if (b.required !== n.required) details.push(`It is now ${n.required ? "required" : "optional"}.`);
    if (b.help !== n.help) details.push("Its help text changed.");
    if (b.pattern !== n.pattern || b.max_length !== n.max_length) details.push("Its checks changed.");
    if (JSON.stringify(b.options ?? null) !== JSON.stringify(n.options ?? null)) details.push("Its choices changed.");
    if (b.default !== n.default) details.push("Its default changed.");
    if (details.length === 0) continue;
    out.push({ part: `input:${key}`, group: "inputs", label, kind: "changed", attention: true, detail: details.join(" ") });
  }
}

/** A version's minimum Aurora version, as numbers; null when it does not parse (then "changed" counts as raised). */
function semver(v: string | null | undefined): number[] | null {
  if (v == null) return null;
  const parts = v.split(".").map((p) => Number.parseInt(p, 10));
  return parts.length > 0 && parts.every((p) => Number.isFinite(p)) ? parts : null;
}

/** Whether `next` is a later version than `base`: the first component that differs decides (a missing one is 0). */
function isLater(next: number[], base: number[]): boolean {
  for (let i = 0; i < Math.max(next.length, base.length); i++) {
    const d = (next[i] ?? 0) - (base[i] ?? 0);
    if (d !== 0) return d > 0;
  }
  return false;
}

function diffMemories(base: TemplateContentView, next: TemplateContentView, out: TemplateChange[]) {
  const keys = new Map<string, { b?: (typeof base.memories)[number]; n?: (typeof next.memories)[number] }>();
  for (const m of base.memories) keys.set(m.key, { b: m });
  for (const m of next.memories) {
    const e = keys.get(m.key) ?? {};
    e.n = m;
    keys.set(m.key, e);
  }
  for (const [key, e] of keys) {
    const label = `Memory ${key}`;
    if (!e.b) {
      out.push({ part: `memory:${key}`, group: "memories", label, kind: "added", attention: false, new: e.n!.content, detail: e.n!.locked ? "New in this version, locked." : "New in this version." });
      continue;
    }
    if (!e.n) {
      out.push({ part: `memory:${key}`, group: "memories", label, kind: "removed", attention: false, old: e.b.content, detail: "Not in this version any more." });
      continue;
    }
    const b = e.b;
    const n = e.n;
    if (b.content !== n.content || b.locked !== n.locked) {
      const c: TemplateChange = {
        part: `memory:${key}`,
        group: "memories",
        label,
        kind: "changed",
        // Unlocking widens what the agent may change.
        attention: b.locked && !n.locked,
        ...(b.content !== n.content ? { old: b.content, new: n.content } : {}),
        detail: b.locked !== n.locked ? (n.locked ? "Now locked: the agent can read it but not change it." : "No longer locked.") : undefined,
      };
      out.push(c);
    }
  }
}

/** Diffs the current version against the one under review (FIX-247): every part, in page order. */
export function diffTemplateContent(base: TemplateContentView, next: TemplateContentView, appName: AppNamer = (id) => id): TemplateDiff {
  const changes: TemplateChange[] = [];

  for (const [part, group, label, b, n] of [
    ["name", "name", "Name", base.name, next.name],
    ["summary", "summary", "Summary", base.summary, next.summary],
    ["readme", "readme", "About this template", base.readme, next.readme],
    ["agent_name", "agent_name", "Agent name", base.agent.name, next.agent.name],
  ] as const) {
    const c = textChange(part, group, label, b, n);
    if (c) changes.push(c);
  }
  if (base.agent.may_message_first !== next.agent.may_message_first) {
    changes.push({ part: "may_message_first", group: "may_message_first", label: "May message first", kind: "changed", attention: false, detail: `It ${next.agent.may_message_first ? "may" : "may not"} message first.` });
  }
  if (base.effort !== next.effort) {
    changes.push({ part: "effort", group: "effort", label: "Effort", kind: "changed", attention: false, detail: `The effort changes from ${base.effort ?? "the default"} to ${next.effort ?? "the default"}.` });
  }
  const instructions = textChange("instructions", "instructions", "Instructions", base.instructions, next.instructions);
  if (instructions) changes.push(instructions);

  // Skills, keyed by name: a renamed skill is a removal and an addition.
  {
    const names = new Map<string, { b?: (typeof base.skills)[number]; n?: (typeof next.skills)[number] }>();
    for (const s of base.skills) names.set(s.name, { b: s });
    for (const s of next.skills) {
      const e = names.get(s.name) ?? {};
      e.n = s;
      names.set(s.name, e);
    }
    for (const [name, e] of names) {
      if (!e.b) {
        changes.push({ part: `skill:${name}`, group: "skills", label: `Skill ${name}`, kind: "added", attention: false, new: e.n!.body, useItWhen: e.n!.use_it_when, detail: "New in this version." });
      } else if (!e.n) {
        changes.push({ part: `skill:${name}`, group: "skills", label: `Skill ${name}`, kind: "removed", attention: false, old: e.b.body, useItWhen: e.b.use_it_when, detail: "Not in this version any more." });
      } else {
        const body = textChange(`skill:${name}:body`, "skills", `Skill ${name}`, e.b.body, e.n.body);
        if (body) changes.push(body);
        const use = textChange(`skill:${name}:use_it_when`, "skills", `Skill ${name} · Use it when`, e.b.use_it_when, e.n.use_it_when);
        if (use) changes.push(use);
      }
      const files = new Map<string, { b?: (typeof base.skills)[number]["files"][number]; n?: (typeof next.skills)[number]["files"][number] }>();
      for (const f of e.b?.files ?? []) files.set(f.path, { b: f });
      for (const f of e.n?.files ?? []) {
        const fe = files.get(f.path) ?? {};
        fe.n = f;
        files.set(f.path, fe);
      }
      for (const [path, fe] of files) {
        const c = fileChange(`skill:${name}:file:${path}`, "skills", `Skill ${name} · ${path}`, fe.b, fe.n);
        if (c) changes.push(c);
      }
    }
  }

  diffSkillRefs(base, next, changes);

  {
    const paths = new Map<string, { b?: (typeof base.helpers)[number]; n?: (typeof next.helpers)[number] }>();
    for (const f of base.helpers) paths.set(f.path, { b: f });
    for (const f of next.helpers) {
      const e = paths.get(f.path) ?? {};
      e.n = f;
      paths.set(f.path, e);
    }
    for (const [path, e] of paths) {
      const c = fileChange(`helper:${path}`, "helpers", `Helper file ${path}`, e.b, e.n, !e.b);
      if (c) changes.push(c);
    }
  }

  diffMemories(base, next, changes);
  diffAutomations(base, next, changes);
  diffApps(base, next, appName, changes);
  diffInputs(base, next, changes);

  if (base.min_core !== next.min_core) {
    const b = semver(base.min_core);
    const n = semver(next.min_core);
    const raised = b == null || n == null ? true : isLater(n, b);
    changes.push({ part: "min_core", group: "min_core", label: "Minimum Aurora version", kind: "changed", attention: raised, detail: `The oldest Aurora this version works with changes from ${base.min_core ?? "none"} to ${next.min_core ?? "none"}.` });
  }
  if (base.has_avatar !== next.has_avatar) {
    changes.push({ part: "avatar", group: "avatar", label: "Avatar", kind: next.has_avatar ? "added" : "removed", attention: false, detail: next.has_avatar ? "The template now carries an avatar." : "The template no longer carries an avatar." });
  }

  return {
    changes,
    summary: summarize(changes),
    attention: changes.filter((c) => c.attention),
    unchanged: unchangedGroups(base, next, changes),
  };
}

/** The phrase one group contributes to the summary line ("Instructions", "2 skills", "a new app"). */
function groupPhrase(group: string, changes: TemplateChange[]): string {
  const n = changes.length;
  const count = (one: string, many: string, n2 = n) => (n2 === 1 ? `1 ${one}` : `${n2} ${many}`);
  switch (group) {
    case "name":
      return "the name";
    case "summary":
      return "the summary";
    case "readme":
      return "About this template";
    case "agent_name":
      return "the agent's name";
    case "may_message_first":
      return "whether it may message first";
    case "instructions":
      return "Instructions";
    case "skills": {
      // A skill can change in several ways (body, a file); it is still one skill.
      const names = new Set(changes.map((c) => c.part.split(":")[1]));
      return count("skill", "skills", names.size);
    }
    case "skill_refs":
      return count("library skill", "library skills");
    case "helpers":
      return n === 1 && changes[0].kind === "added" ? "a new helper file" : count("helper file", "helper files");
    case "memories":
      return count("memory", "memories");
    case "automations": {
      const schedules = changes.filter((c) => c.part.startsWith("automation:schedule:"));
      const webhooks = changes.filter((c) => c.part.startsWith("automation:webhook:"));
      const phrase = (list: TemplateChange[], noun: string) =>
        list.length === 0
          ? null
          : list.length === 1 && list[0].kind === "added"
            ? `a new ${noun}`
            : list.length === 1
              ? `1 ${noun}`
              : `${list.length} ${noun}s`;
      return [phrase(schedules, "schedule"), phrase(webhooks, "webhook")].filter((p): p is string => p != null).join(", ");
    }
    case "apps":
      return n === 1 && changes[0].kind === "added" ? "a new app" : count("app", "apps");
    case "inputs":
      return n === 1 && changes[0].kind === "added" ? "a new input" : count("input", "inputs");
    case "effort":
      return "the effort";
    case "min_core":
      return "the minimum Aurora version";
    case "avatar":
      return "the avatar";
    default:
      return group;
  }
}

function summarize(changes: TemplateChange[]): string {
  if (changes.length === 0) return "";
  const groups: string[] = [];
  for (const c of changes) {
    if (!groups.includes(c.group)) groups.push(c.group);
  }
  // The phrase order is the review page's part order, not the order the changes were found in.
  const order = ["instructions", "skills", "skill_refs", "helpers", "memories", "apps", "automations", "inputs", "name", "summary", "readme", "agent_name", "may_message_first", "effort", "min_core", "avatar"];
  groups.sort((a, b) => order.indexOf(a) - order.indexOf(b));
  const phrases = groups
    .map((g) => groupPhrase(g, changes.filter((c) => c.group === g)))
    .filter((p) => p !== "");
  const n = changes.length;
  return `${n} ${n === 1 ? "change" : "changes"}: ${phrases.join(", ")}`;
}

/** The part groups with no change, as the "Unchanged:" labels, in the review page's order. */
function unchangedGroups(base: TemplateContentView, next: TemplateContentView, changes: TemplateChange[]): string[] {
  const touched = new Set(changes.map((c) => c.group));
  const withCount = (label: string, n: number) => (n > 0 ? `${label} (${n})` : label);
  const rows: [string, string, number][] = [
    ["name", "Name", 0],
    ["summary", "Summary", 0],
    ["readme", "About this template", 0],
    ["agent_name", "Agent name", 0],
    ["may_message_first", "May message first", 0],
    ["effort", "Effort", 0],
    ["instructions", "Instructions", 0],
    ["skills", "Skills", next.skills.length],
    ["skill_refs", "Skills from libraries", (next.skill_refs as TemplateSkillRefView[]).length],
    ["helpers", "Helper files", next.helpers.length],
    ["memories", "Memories", next.memories.length],
    ["automations", "Automations", (next.automations as TemplateAutomationView[]).length],
    ["apps", "Apps", (next.apps as TemplateAppEntryView[]).length],
    ["inputs", "Inputs", (next.inputs as TemplateInputView[]).length],
    ["min_core", "Minimum Aurora version", 0],
    ["avatar", "Avatar", 0],
  ];
  return rows.filter(([group]) => !touched.has(group)).map(([, label, n]) => withCount(label, n));
}
