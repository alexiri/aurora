import { StatusDot } from "@/components/ui/status-dot";
import { LIBRARY_STATUS, label } from "@/lib/labels";
import type { SkillLibrary } from "@/lib/api/skill-libraries";

const TONE = { active: "accent", paused: "neutral", error: "danger", over_budget: "pending" } as const;

/** A library migrated without an address has nothing to read until someone enters it (C-1127). */
export function needsAddress(l: SkillLibrary): boolean {
  return l.status === "error" && l.url === null;
}

/** The reader's sanitized reason says the repository wants a token (C-1125: "The repository needs an access token"). */
export function needsToken(l: SkillLibrary): boolean {
  return l.status === "error" && /needs an access token/i.test(l.last_error ?? "");
}

/**
 * The status in words (D-450(e), C-1129): Active, Paused, "Can't be read" with the reason, "Needs an access token",
 * "Pending updates over budget"; a migrated row without an address says what to enter. The reason is repository text:
 * data, never markup.
 */
export function LibraryStatus({ library: l }: { library: SkillLibrary }) {
  const tone = TONE[l.status as keyof typeof TONE] ?? "danger";
  const text = needsToken(l) ? "Needs an access token" : label(LIBRARY_STATUS, l.status);
  // The word already says it for a missing token; a migrated row without an address says what to enter.
  const reason = needsAddress(l) ? "Enter the repository's git address" : l.status === "error" && !needsToken(l) ? l.last_error : null;
  return (
    <div className="min-w-0">
      <StatusDot tone={tone}>{text}</StatusDot>
      {reason ? <p className="mt-1 max-w-xs whitespace-normal text-xs text-danger-ink [overflow-wrap:anywhere]">{reason}</p> : null}
    </div>
  );
}
