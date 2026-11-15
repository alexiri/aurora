import type { Attachment } from "@/lib/api/attachments";

/**
 * Composer drafts (P5-CHAT-3, docs/14 "Drafts"): per signed-in user and conversation, the text, the reply target and
 * the uploaded-but-unsent attachments, in the browser's local storage. Every access is wrapped: a browser without
 * storage (private window, blocked site data, quota) just has no drafts. The key carries the user id so a second
 * person on the same browser never sees the first one's text, and `clearDrafts()` runs on sign-out.
 */
export interface Draft {
  text: string;
  reply: { seq: number; author: string; excerpt: string; at?: string } | null;
  attachments: Attachment[];
}

const PREFIX = "aurora:chat-draft:";
const PREF_FORMATTING = "aurora:chat-formatting";

export const draftKey = (userId: string, convId: string) => `${PREFIX}${userId}:${convId}`;

export function loadDraft(userId: string, convId: string): Draft | null {
  try {
    const raw = window.localStorage.getItem(draftKey(userId, convId));
    if (!raw) return null;
    const v: unknown = JSON.parse(raw);
    if (typeof v !== "object" || v === null) return null;
    const d = v as Partial<Draft>;
    return {
      text: typeof d.text === "string" ? d.text : "",
      reply: d.reply && typeof d.reply.seq === "number" ? d.reply : null,
      attachments: Array.isArray(d.attachments) ? d.attachments.filter((a) => a && typeof a.id === "string") : [],
    };
  } catch {
    return null;
  }
}

/** An empty draft removes the entry. */
export function saveDraft(userId: string, convId: string, d: Draft): void {
  try {
    const key = draftKey(userId, convId);
    if (!d.text && !d.reply && d.attachments.length === 0) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, JSON.stringify(d));
  } catch {
    /* no storage: the composer works without drafts */
  }
}

export function clearDraft(userId: string, convId: string): void {
  try {
    window.localStorage.removeItem(draftKey(userId, convId));
  } catch {
    /* ignored */
  }
}

/** Sign-out: every draft of every user on this browser goes. */
export function clearDrafts(): void {
  try {
    const s = window.localStorage;
    for (const k of Object.keys(s)) if (k.startsWith(PREFIX)) s.removeItem(k);
  } catch {
    /* ignored */
  }
}

/** The `Aa` toggle: `null` = never chosen (the row shows from 768 px and not below). */
export function loadFormattingPref(): boolean | null {
  try {
    const v = window.localStorage.getItem(PREF_FORMATTING);
    return v === "1" ? true : v === "0" ? false : null;
  } catch {
    return null;
  }
}

export function saveFormattingPref(on: boolean): void {
  try {
    window.localStorage.setItem(PREF_FORMATTING, on ? "1" : "0");
  } catch {
    /* ignored */
  }
}
