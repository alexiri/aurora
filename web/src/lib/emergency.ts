// (FIX-236, D-478, C-1165) The words for emergency read access, in one place so the reader's banner,
// the workspace people's banner, the chat's visibility line and the history sections all say the same
// thing. Every helper is a pure function of its arguments (plus the person's display time zone), so the
// tests pin the sentences exactly. The texts never cite a document or contract id.
//
// The banner is metadata only: the reason is the opener's own sanitized words, and no conversation
// content is ever in an EmergencyAccess row (C-1164).

import type { components } from "@/lib/api/schema";
import { formatDay, formatTime } from "@/lib/format-date";
import { label, type LabelTable } from "@/lib/labels";
import { personName } from "@/lib/workspace-name";

type S = components["schemas"];
/** `Workspace.audience.emergency` (C-1165): the newest row that is active or ended within the last 7 days. */
export type WorkspaceEmergency = NonNullable<S["WorkspaceAudience"]["emergency"]>;
/** One history row (C-1164): metadata only — who, why, from, until, how it ended. */
export type EmergencyAccess = S["EmergencyAccess"];

/**
 * How a row ended, in words. The keys are `workspace_emergency_access.end_reason` (C-1164, a CHECK
 * constraint, not a schema enum, so nothing to add to `COVERED`).
 */
export const EMERGENCY_END = {
  ended: { label: "Ended" },
  expired: { label: "Expired" },
  access_lost: { label: "Access lost" },
} satisfies LabelTable;

/**
 * (FIX-277) The one question every page asks of a workspace answer: does the viewer read it through their own emergency
 * grant? Then it draws nothing that acts and asks for nothing the grant doesn't cover (C-1165).
 */
export function isEmergencyReader(workspace: { emergency_reader?: boolean } | null | undefined): boolean {
  return workspace?.emergency_reader === true;
}

/** The workspace topics emergency read access opens (C-1164): the rail's set, and the pages that exist for a reader. */
export const EMERGENCY_TOPICS = ["overview", "agents", "skills", "activity", "people"] as const;

/**
 * Whether a page under `/o/<org>/w/<ws>` is one a reader may open; `rest` is the address after the workspace. Of the
 * agents' pages, creating one from a template and the terminal are not.
 */
export function emergencyCovers(rest: string): boolean {
  const [topic = "", second, third] = rest.split("/").filter(Boolean);
  if (topic === "") return true;
  if (topic === "agents") return second !== "new" && third === undefined;
  return (EMERGENCY_TOPICS as readonly string[]).includes(topic) && second === undefined;
}

/** (FIX-280) Under a skill's file tree for a reader: the server keeps a skill's files to people with their own access. */
export const EMERGENCY_NO_SKILL_FILES = "Emergency read access doesn’t cover a skill’s files.";

/** The reader's banner (brief 2): on every page while the reader holds the grant. */
export function emergencyReaderBannerText(expiresAt: string): string {
  return `Emergency read access until ${formatTime(expiresAt)}. Everything you open is recorded, and this workspace's people have been told.`;
}

/** The manage page, while the viewer's own grant is open (FIX-277); without an end time it says only that it is open. */
export function emergencyOpenText(expiresAt: string | null): string {
  return expiresAt ? `Emergency read access is open until ${formatTime(expiresAt)}.` : "Emergency read access is open.";
}

/** "14:00 to 18:00", or "19 Nov, 14:00 to 20 Nov, 18:00" when the days differ. */
export function emergencySpanText(from: string, to: string | null | undefined): string {
  const start = formatDay(from);
  const end = to ? formatDay(to) : null;
  const sameDay = end !== null && end === start;
  const fromText = sameDay ? formatTime(from) : `${start}, ${formatTime(from)}`;
  const toText = to ? (sameDay ? formatTime(to) : `${end}, ${formatTime(to)}`) : formatTime(from);
  return `${fromText} to ${toText}`;
}

/**
 * The workspace people's banner (brief 3), while it is active and for 7 days after it ends
 * ("has … until" / "had … from–to"). `activeMore` is how many OTHER emergency grants are active
 * right now (review HIGH-1: two simultaneous openers are both announced).
 */
export function emergencyPeopleBannerText(e: WorkspaceEmergency, activeMore = 0): string {
  const name = personName(e.by.display_name);
  // The reason is the opener's own words: it may already end its sentence.
  const said = e.reason.trim();
  const reason = /[.!?…]$/.test(said) ? said : `${said}.`;
  const more =
    activeMore > 0
      ? `, along with ${activeMore} more organization owner${activeMore === 1 ? "" : "s"}`
      : "";
  if (e.active) {
    return `${name}, an organization owner, has emergency read access to this workspace until ${formatTime(e.expires_at)}${more}. Reason: ${reason}`;
  }
  return `${name}, an organization owner, had emergency read access from ${emergencySpanText(e.opened_at, e.ended_at ?? e.expires_at)}. Reason: ${reason}`;
}

/** The chat's extra visibility line (brief 4, D-479's table). */
export function emergencyVisibilityLine(e: WorkspaceEmergency): string {
  return `${personName(e.by.display_name)}, an organization owner, can read this workspace's conversations until ${formatTime(e.expires_at)} (emergency access).`;
}

/** How a history row ended, in words: "Until 18:00" while it is active, else the end label. */
export function emergencyEndText(row: Pick<EmergencyAccess, "active" | "expires_at" | "end_reason">): string {
  if (row.active) return `Until ${formatTime(row.expires_at)}`;
  return label(EMERGENCY_END, row.end_reason);
}

/* ---- the per-person dismissal (brief 3: local storage, with try/catch) ---- */

const dismissKey = (id: string) => `aurora.emergency-seen.${id}`;

/** True when this browser dismissed the banner for this grant. Unavailable storage reads as "not dismissed". */
export function emergencyDismissed(id: string): boolean {
  try {
    return window.localStorage.getItem(dismissKey(id)) === "1";
  } catch {
    return false;
  }
}

/** Remember the dismissal for this grant; a blocked storage keeps the banner (nothing throws). */
export function dismissEmergency(id: string): void {
  try {
    window.localStorage.setItem(dismissKey(id), "1");
  } catch {
    // Storage is blocked: the banner stays and can be dismissed again next time.
  }
}
