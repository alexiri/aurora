"use client";

import { useState, useSyncExternalStore } from "react";

import { PushDevices } from "@/components/account/push-devices";
import { Button } from "@/components/ui/button";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { useUpdateMe } from "@/lib/api/account";
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
  // (account-1) Blocked is news only after the person asked for notifications: they just tried, or the switch is already on.
  const wanted = useSyncExternalStore(noop, read(() => isNotifyEnabled()), () => false);
  const [tried, setTried] = useState(false);

  async function toggleBrowser(on: boolean) {
    if (!on) setNotifyEnabled(false);
    else {
      setTried(true);
      await enableBrowserNotifications();
    }
    bump((n) => n + 1);
  }

  return (
    <>
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
            Only the title and a link to the dashboard are sent.
          </span>
        </span>
      </label>
      {/* (FIX-167 Part B, C-1024) Instance admins only; the push follows the device's own opt-in below. */}
      {me.user.is_instance_admin ? (
        <label className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            className="mt-1 size-4 accent-accent"
            checked={me.admin_attention_email}
            disabled={update.isPending}
            onChange={(e) => update.mutate({ admin_attention_email: e.target.checked })}
          />
          <span>
            E-mail me when something needs my decision in Administration
            <span className="block text-xs text-muted">
              One e-mail for each new connector, app package or key change that waits for you. It names the item and links to
              Administration, never a setting or a secret.
            </span>
          </span>
        </label>
      ) : null}
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
      {denied && (tried || wanted) ? (
        <Notice tone="danger" role="status">
          Notifications are blocked for this site. Allow them in the browser settings, then try again.
        </Notice>
      ) : null}
      <ErrorNotice error={update.error} />
      {browser ? (
        <Button type="button" variant="ghost" size="sm" onClick={() => void toggleBrowser(false)}>
          Turn off browser notifications
        </Button>
      ) : null}
    </div>
    {/* (P4-011B-4, C-836) Web Push for this device and the device list; nothing while push is not available. */}
    <PushDevices />
    </>
  );
}
