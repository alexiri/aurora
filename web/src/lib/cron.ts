/**
 * (P5-REFINE-14, D-405) Cron in words. The Automations page never shows a raw cron outside Technical details: a schedule
 * is one of the presets of the form (Every 5 minutes, Hourly, Every day, Weekdays, Weekly) or "Custom schedule". Pure
 * functions, no React: the form writes a cron from a preset, Edit reads the preset back, the row writes the sentence, and
 * the Custom field is flagged as you type when it runs more often than the API's minimum (`MIN_GAP_MINUTES`).
 */

/** The API's shortest interval between two runs (C-1077 `min_interval_secs: 300`). */
export const MIN_GAP_MINUTES = 5;

export type SchedulePreset = "every5" | "hourly" | "daily" | "weekdays" | "weekly" | "custom";

/** The presets in the order the form shows them. There is none shorter than five minutes. */
export const PRESETS: readonly { value: SchedulePreset; label: string }[] = [
  { value: "every5", label: "Every 5 minutes" },
  { value: "hourly", label: "Hourly" },
  { value: "daily", label: "Every day" },
  { value: "weekdays", label: "Weekdays" },
  { value: "weekly", label: "Weekly" },
  { value: "custom", label: "Custom" },
];

/** 0 = Sunday … 6 = Saturday, as cron counts them. */
export const WEEKDAY_NAMES = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"] as const;

export interface ScheduleSpec {
  preset: SchedulePreset;
  /** 0–59: the minute of "Hourly", the minute of the time of day for the others. */
  minute: number;
  /** 0–23: the hour of "Every day", "Weekdays" and "Weekly". */
  hour: number;
  /** 0–6 (Sunday = 0): the day of "Weekly". */
  weekday: number;
}

export const DEFAULT_SPEC: ScheduleSpec = { preset: "every5", minute: 0, hour: 9, weekday: 1 };

const pad = (n: number): string => String(n).padStart(2, "0");

/** "09:00". */
export const clock = (hour: number, minute: number): string => `${pad(hour)}:${pad(minute)}`;

function int(s: string, min: number, max: number): number | null {
  if (!/^\d{1,2}$/.test(s)) return null;
  const n = Number(s);
  return n >= min && n <= max ? n : null;
}

/** The cron a preset stands for; `custom` has none (the form keeps the person's own text). */
export function specToCron(spec: ScheduleSpec): string | null {
  const m = spec.minute;
  switch (spec.preset) {
    case "every5":
      return "*/5 * * * *";
    case "hourly":
      return `${m} * * * *`;
    case "daily":
      return `${m} ${spec.hour} * * *`;
    case "weekdays":
      return `${m} ${spec.hour} * * 1-5`;
    case "weekly":
      return `${m} ${spec.hour} * * ${spec.weekday}`;
    default:
      return null;
  }
}

/** Reads a preset back from a cron (so Edit opens on the right one); anything else is `custom`. */
export function cronToSpec(cron: string | null | undefined): ScheduleSpec {
  const custom: ScheduleSpec = { ...DEFAULT_SPEC, preset: "custom" };
  if (!cron) return custom;
  const f = cron.trim().split(/\s+/);
  if (f.length !== 5) return custom;
  const [mi, ho, dom, mon, dow] = f;
  if (dom !== "*" || mon !== "*") return custom;
  if (mi === "*/5" && ho === "*" && dow === "*") return { ...DEFAULT_SPEC, preset: "every5" };
  const minute = int(mi, 0, 59);
  if (minute === null) return custom;
  if (ho === "*" && dow === "*") return { ...DEFAULT_SPEC, preset: "hourly", minute };
  const hour = int(ho, 0, 23);
  if (hour === null) return custom;
  if (dow === "*") return { ...DEFAULT_SPEC, preset: "daily", minute, hour };
  if (dow === "1-5") return { ...DEFAULT_SPEC, preset: "weekdays", minute, hour };
  const weekday = int(dow, 0, 6);
  if (weekday !== null) return { ...DEFAULT_SPEC, preset: "weekly", minute, hour, weekday };
  return custom;
}

// The values a cron field lists (a star, a star with a step, a number, a range, a range with a step, a number with a step,
// comma lists), or `null` when it is not understood.
function fieldValues(field: string, min: number, max: number): number[] | null {
  const out = new Set<number>();
  for (const part of field.split(",")) {
    const [range, stepText, extra] = part.split("/");
    if (extra !== undefined || range === "") return null;
    let step = 1;
    if (stepText !== undefined) {
      const s = int(stepText, 1, max);
      if (s === null) return null;
      step = s;
    }
    let lo: number | null;
    let hi: number | null;
    if (range === "*") {
      lo = min;
      hi = max;
    } else if (range.includes("-")) {
      const [a, b, more] = range.split("-");
      if (more !== undefined) return null;
      lo = int(a, min, max);
      hi = int(b, min, max);
    } else {
      lo = int(range, min, max);
      hi = stepText !== undefined ? max : lo;
    }
    if (lo === null || hi === null || lo > hi) return null;
    for (let v = lo; v <= hi; v += step) out.add(v);
  }
  return [...out].sort((a, b) => a - b);
}

/**
 * The smallest number of minutes between two runs of `cron`, or `null` when it cannot be told (not five fields, or a
 * field this function does not understand). Conservative: it only looks at the minute field (lists, ranges and steps
 * included) and the hour field (to know whether the hours wrap), so it never flags a cron the API accepts. The API stays
 * the authority; this is what lets the form say "Too often" while you type.
 */
export function minGapMinutes(cron: string): number | null {
  const f = cron.trim().split(/\s+/);
  if (f.length !== 5) return null;
  const minutes = fieldValues(f[0], 0, 59);
  const hours = fieldValues(f[1], 0, 23);
  if (!minutes || !hours || minutes.length === 0 || hours.length === 0) return null;
  let gap = 60;
  for (let i = 1; i < minutes.length; i++) gap = Math.min(gap, minutes[i] - minutes[i - 1]);
  // Across the hour: the last minute of one firing hour to the first of the next, when two firing hours follow each other.
  const consecutiveHours = hours.length > 1 && hours.some((h) => hours.includes((h + 1) % 24));
  if (minutes.length > 1 && consecutiveHours) gap = Math.min(gap, 60 - minutes[minutes.length - 1] + minutes[0]);
  return gap;
}

/** True when `cron` surely runs more often than the API allows. */
export function isTooFrequent(cron: string): boolean {
  const g = minGapMinutes(cron);
  return g !== null && g < MIN_GAP_MINUTES;
}

/** "Zurich" for `Europe/Zurich`, "Buenos Aires" for `America/Argentina/Buenos_Aires`, "UTC" for `UTC`. */
export function zoneCity(timezone: string | null | undefined): string {
  if (!timezone) return "";
  const last = timezone.split("/").pop() ?? timezone;
  return last.replace(/_/g, " ");
}

/**
 * The cadence as a sentence: "Every 5 minutes", "Every hour at minute 15", "Every day at 09:00", "Every weekday at 09:00",
 * "Every Monday at 09:00"; `null` for a custom cron (the caller writes "Custom schedule").
 */
export function cadenceWords(cron: string | null | undefined): string | null {
  const s = cronToSpec(cron);
  switch (s.preset) {
    case "every5":
      return "Every 5 minutes";
    case "hourly":
      return s.minute === 0 ? "Every hour" : `Every hour at minute ${s.minute}`;
    case "daily":
      return `Every day at ${clock(s.hour, s.minute)}`;
    case "weekdays":
      return `Every weekday at ${clock(s.hour, s.minute)}`;
    case "weekly":
      return `Every ${WEEKDAY_NAMES[s.weekday]} at ${clock(s.hour, s.minute)}`;
    default:
      return null;
  }
}

/** The row's "when": "Every weekday at 09:00 · Zurich"; a cron the presets do not cover reads "Custom schedule". */
export function whenWords(cron: string | null | undefined, timezone?: string | null): string {
  const words = cadenceWords(cron);
  if (!words) return "Custom schedule";
  // A cadence that does not depend on a wall clock needs no zone.
  const spec = cronToSpec(cron);
  const city = spec.preset === "every5" || spec.preset === "hourly" ? "" : zoneCity(timezone);
  return city ? `${words} · ${city}` : words;
}
