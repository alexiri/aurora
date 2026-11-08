// Browser notifications for approvals (phase 2, RFE-011a, docs/14 §Approval notifications).
// Opt-in, stored in this browser only, tab-bound (not Web Push: that is RFE-011b).

import type { components } from "@/lib/api/schema";

type Event = components["schemas"]["ApprovalEvent"];
type Notice = NonNullable<Event["notice"]>;

export const NOTIFY_STORAGE_KEY = "aurora.approvalNotify";

export function isNotifyEnabled(): boolean {
  try {
    return window.localStorage.getItem(NOTIFY_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

export function setNotifyEnabled(on: boolean): void {
  try {
    if (on) window.localStorage.setItem(NOTIFY_STORAGE_KEY, "1");
    else window.localStorage.removeItem(NOTIFY_STORAGE_KEY);
  } catch {
    /* storage unavailable: the choice just does not persist */
  }
}

export function notificationsSupported(): boolean {
  return typeof window !== "undefined" && "Notification" in window;
}

/** Must be called from a user gesture. Stores the opt-in only when permission is granted. */
export async function enableBrowserNotifications(): Promise<NotificationPermission> {
  if (!notificationsSupported()) return "denied";
  const permission = await Notification.requestPermission();
  setNotifyEnabled(permission === "granted");
  return permission;
}

/** A notification appears only for a created approval, with permission, opt-in and a tab the user is not looking at. */
export function shouldNotify(o: {
  event: string;
  notice: Notice | null | undefined;
  enabled: boolean;
  permission: NotificationPermission | "unsupported";
  hidden: boolean;
  focused: boolean;
}): boolean {
  return (
    o.event === "created" &&
    !!o.notice &&
    o.enabled &&
    o.permission === "granted" &&
    (o.hidden || !o.focused)
  );
}

export function noticeUrl(n: Notice): string {
  return `/o/${n.workspace.org_slug}/w/${n.workspace.slug}/approvals?focus=${n.id}`;
}

/** Agent, workspace and the sanitized title; `tag = approval id` so a repeat never duplicates. */
export function notificationParts(n: Notice): { title: string; options: { body: string; tag: string } } {
  const agent = n.agent?.name ?? "An agent";
  return {
    title: "Approval needed",
    options: { body: `${agent} · ${n.workspace.name}\n${n.title}`, tag: n.id },
  };
}
