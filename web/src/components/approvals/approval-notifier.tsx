"use client";

import { useEffect } from "react";

import { parseApprovalEvent } from "@/lib/api/approvals";
import { isNotifyEnabled, noticeUrl, notificationParts, shouldNotify } from "@/lib/approval-notify";

/**
 * Raises a browser notification for a new approval the user can decide (the stream only carries
 * those) while the tab is hidden or unfocused. Mounted once in the app shell; opens its own
 * stream only when the user opted in on this browser and the permission is granted.
 */
export function ApprovalNotifier() {
  useEffect(() => {
    if (typeof EventSource === "undefined" || typeof Notification === "undefined") return;
    if (!isNotifyEnabled() || Notification.permission !== "granted") return;
    const es = new EventSource("/v1/approvals/stream");
    es.addEventListener("approval", (e) => {
      const evt = parseApprovalEvent((e as MessageEvent<string>).data);
      if (!evt) return;
      const ok = shouldNotify({
        event: evt.event,
        notice: evt.notice,
        enabled: isNotifyEnabled(),
        permission: Notification.permission,
        hidden: document.hidden,
        focused: document.hasFocus(),
      });
      if (!ok || !evt.notice) return;
      const { title, options } = notificationParts(evt.notice);
      const url = noticeUrl(evt.notice);
      const n = new Notification(title, options);
      n.onclick = () => {
        window.focus();
        window.location.assign(url);
        n.close();
      };
    });
    return () => es.close();
  }, []);
  return null;
}
