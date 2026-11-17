/**
 * A plain-language reading of the common cron shapes for the schedule dialog's preview. It only describes; the API
 * validates the expression and the one-hour minimum and stays authoritative. An expression it does not recognise
 * gets `null` and the dialog says the exact times appear after saving.
 */

const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

const pad = (n: number) => String(n).padStart(2, "0");
const isNum = (s: string) => /^\d+$/.test(s);

function days(field: string): string | null {
  if (field === "*") return "day";
  const parts = field.split(",");
  const names: string[] = [];
  for (const p of parts) {
    const range = /^(\d)-(\d)$/.exec(p);
    if (range) {
      const a = Number(range[1]) % 7;
      const b = Number(range[2]) % 7;
      if (b < a) return null;
      names.push(a === b ? DAYS[a]! : `${DAYS[a]} to ${DAYS[b]}`);
    } else if (/^\d$/.test(p)) names.push(DAYS[Number(p) % 7]!);
    else return null;
  }
  return names.length === 1 ? names[0]! : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

export function describeCron(cron: string, timezone: string): string | null {
  const f = cron.trim().split(/\s+/);
  if (f.length !== 5) return null;
  const [min, hour, dom, mon, dow] = f as [string, string, string, string, string];
  const tz = timezone.trim() ? ` (${timezone.trim()})` : "";
  if (mon !== "*") return null;

  // Every hour, or every n hours, on the minute.
  if (isNum(min) && Number(min) < 60 && dom === "*" && dow === "*") {
    if (hour === "*") return `Every hour, at ${min} minutes past${tz}.`;
    const step = /^\*\/(\d+)$/.exec(hour);
    if (step && Number(step[1]) > 0 && Number(step[1]) < 24) return `Every ${step[1]} hours, at ${min} minutes past${tz}.`;
  }
  if (isNum(min) && isNum(hour) && Number(min) < 60 && Number(hour) < 24) {
    const at = `${pad(Number(hour))}:${pad(Number(min))}${tz}`;
    if (dom === "*") {
      const d = days(dow);
      if (d === null) return null;
      return d === "day" ? `Every day at ${at}.` : `Every ${d} at ${at}.`;
    }
    if (isNum(dom) && dow === "*" && Number(dom) >= 1 && Number(dom) <= 31) return `On day ${dom} of every month at ${at}.`;
  }
  return null;
}

/**
 * Whether the expression can fire more than once an hour: the minute field is a list, a range or a step, or an hour
 * field that is `*` with a `*` minute. A hint only (the API refuses it with 422 `invalid`).
 */
export function firesMoreThanHourly(cron: string): boolean {
  const f = cron.trim().split(/\s+/);
  if (f.length !== 5) return false;
  const min = f[0]!;
  const hour = f[1]!;
  if (/[*,/-]/.test(min)) return true;
  return min === "*" && hour === "*";
}
