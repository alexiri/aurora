"use client";

import { useState, useSyncExternalStore } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { useUpdateMe } from "@/lib/api/account";
import { errorMessage } from "@/lib/api/errors";
import type { Me } from "@/lib/api/queries";
import {
  enableBrowserNotifications,
  isNotifyEnabled,
  notificationsSupported,
  setNotifyEnabled,
} from "@/lib/approval-notify";

/** Account → approval notifications: the e-mail switch and the browser opt-in (RFE-011a). */
export function ApprovalNotifications({ me }: { me: Me }) {
  const update = useUpdateMe();
  const [, bump] = useState(0);
  // Browser-only state (localStorage, Notification permission): false on the server.
  const read = (f: () => boolean) => () => notificationsSupported() && f();
  const noop = () => () => {};
  const supported = useSyncExternalStore(noop, notificationsSupported, () => false);
  const browser = useSyncExternalStore(
    noop,
    read(() => isNotifyEnabled() && Notification.permission === "granted"),
    () => false,
  );
  const denied = useSyncExternalStore(noop, read(() => Notification.permission === "denied"), () => false);

  async function toggleBrowser(on: boolean) {
    if (!on) setNotifyEnabled(false);
    else await enableBrowserNotifications();
    bump((n) => n + 1);
  }

  return (
    <div className="space-y-4 max-w-md">
      <label className="flex items-start gap-2 text-sm">
        <input
          type="checkbox"
          className="mt-1 size-4 accent-accent"
          checked={me.approval_email}
          disabled={update.isPending}
          onChange={(e) => update.mutate({ approval_email: e.target.checked })}
        />
        <span>
          E-mail me approvals nobody is watching
          <span className="block text-xs text-muted">
            Only the title and a link to the dashboard are sent, never a way to decide from the e-mail.
          </span>
        </span>
      </label>
      <label className="flex items-start gap-2 text-sm">
        <input
          type="checkbox"
          className="mt-1 size-4 accent-accent"
          checked={browser}
          disabled={!supported}
          onChange={(e) => void toggleBrowser(e.target.checked)}
        />
        <span>
          Notify me in this browser when an approval needs me
          <span className="block text-xs text-muted">
            Works while a tab is open. Stored in this browser only.
          </span>
        </span>
      </label>
      {!supported ? <p className="text-xs text-muted">This browser does not support notifications.</p> : null}
      {denied ? (
        <Notice tone="danger" role="status">
          Notifications are blocked for this site. Allow them in the browser settings, then try again.
        </Notice>
      ) : null}
      {update.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(update.error)}
        </Notice>
      ) : null}
      {browser ? (
        <Button type="button" variant="ghost" size="sm" onClick={() => void toggleBrowser(false)}>
          Turn off browser notifications
        </Button>
      ) : null}
    </div>
  );
}
