"use client";

// Account › Profile › Notifications: Web Push for this device and the device list (P4-011B-4, D-134, D-266(c),
// C-831, C-836, docs/14 §Web Push). Shown only while `GET /v1/push/config` says `available`. Turn on needs a gesture.
import { useQueryClient } from "@tanstack/react-query";
import { useState, useSyncExternalStore, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import {
  pushKeys,
  usePushConfig,
  usePushSubscriptions,
  useTestPushSubscription,
  useUpdatePushSubscription,
  type PushConfig,
  type PushState,
  type PushSubscriptionRow,
} from "@/lib/api/push";
import { fmtDate as fmt } from "@/lib/utils";
import { deviceLabel, hasPushMarker, pushSupport, removeDevice, turnOnPush, type PushSupport } from "@/lib/web-push";

const STATE_CHIP: Record<PushState, { text: string; tone: "accent" | "neutral" | "pending" | "danger" }> = {
  active: { text: "Active", tone: "accent" },
  paused: { text: "Paused", tone: "neutral" },
  disabled: { text: "Not receiving", tone: "danger" },
  expired: { text: "Not receiving", tone: "danger" },
  stale_key: { text: "Needs update", tone: "pending" },
};

const SUPPORT_TEXT: Record<Exclude<PushSupport, "ok">, string> = {
  unsupported: "This browser cannot receive push notifications.",
  insecure: "Push notifications need a secure connection (https, or localhost). Open Aurora over https to turn them on.",
  ios_home_screen:
    "On iPhone and iPad, push notifications work only for a site added to the Home Screen. Tap Share, choose Add to Home Screen, then open Aurora from there.",
};

const envListeners = new Set<() => void>();
const subscribeEnv = (cb: () => void) => {
  envListeners.add(cb);
  return () => void envListeners.delete(cb);
};
/** Re-read the browser facts after the card changed them (Turn on, Remove). */
const refreshEnv = () => envListeners.forEach((f) => f());
const envSnapshot = () =>
  [pushSupport(), typeof Notification !== "undefined" && Notification.permission === "denied" ? "1" : "0", hasPushMarker() ? "1" : "0"].join("|");
function parseEnv(raw: string): { support: PushSupport; denied: boolean; marker: boolean } {
  const [support, denied, marker] = raw.split("|");
  return { support: support as PushSupport, denied: denied === "1", marker: marker === "1" };
}

function RenameDialog({ target, onClose }: { target: PushSubscriptionRow | null; onClose: () => void }) {
  const update = useUpdatePushSubscription();
  const [label, setLabel] = useState("");
  const [seen, setSeen] = useState<string | null>(null);
  if (target && seen !== target.id) {
    setSeen(target.id);
    setLabel(target.label);
    update.reset();
  }
  if (!target && seen !== null) setSeen(null);
  return (
    <FormDialog
      open={target !== null}
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
      title="Rename device"
      submitLabel="Save"
      busy={update.isPending}
      submitDisabled={label.trim() === ""}
      onSubmit={(e: FormEvent) => {
        e.preventDefault();
        if (target) update.mutate({ id: target.id, label: label.trim() }, { onSuccess: onClose });
      }}
    >
      <Field id="push-device-label" label="Name" error={update.error ? errorMessage(update.error) : undefined}>
        <input
          id="push-device-label"
          className={inputClass}
          value={label}
          maxLength={80}
          autoComplete="off"
          onChange={(e) => setLabel(e.target.value)}
        />
      </Field>
    </FormDialog>
  );
}

function DeviceRow({
  row,
  onRename,
  onRemove,
  onTurnOnAgain,
  busy,
}: {
  row: PushSubscriptionRow;
  onRename: () => void;
  onRemove: () => void;
  onTurnOnAgain: () => void;
  busy: boolean;
}) {
  const update = useUpdatePushSubscription();
  const test = useTestPushSubscription();
  const chip = STATE_CHIP[row.state];
  const notReceiving = row.state === "disabled" || row.state === "expired";
  const error = update.error ?? test.error;
  return (
    <li className="space-y-2 py-3 first:pt-0 last:pb-0" aria-label={row.label}>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className="min-w-0 break-words text-sm font-semibold text-ink">{row.label}</span>
        {row.this_session ? <Badge>This device</Badge> : null}
        <Badge tone={chip.tone}>{chip.text}</Badge>
      </div>
      <p className="text-xs text-muted">
        Added {fmt(row.created_at)} · Last delivery {fmt(row.last_success_at)}
      </p>
      {notReceiving ? (
        <p className="text-xs text-muted">
          {row.this_session
            ? "Aurora could not deliver to this device. Turn it on again."
            : "Aurora could not deliver to this device. Open Aurora there and turn it on again, or remove it."}
        </p>
      ) : null}
      {row.state === "stale_key" ? (
        <p className="text-xs text-muted">The notification key changed. Open Aurora on that device to update it.</p>
      ) : null}
      <label className="flex items-start gap-2 text-sm">
        <input
          type="checkbox"
          className="mt-1 size-4 accent-accent"
          checked={row.show_details}
          disabled={update.isPending}
          onChange={(e) => update.mutate({ id: row.id, show_details: e.target.checked })}
        />
        <span>
          Show agent and workspace names in notifications
          <span className="block text-xs text-muted">Names can appear on a lock screen. Off by default.</span>
        </span>
      </label>
      <div className="flex flex-wrap gap-2">
        {notReceiving && row.this_session ? (
          <Button type="button" size="sm" disabled={busy} onClick={onTurnOnAgain}>
            Turn on again
          </Button>
        ) : null}
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={update.isPending}
          onClick={() => update.mutate({ id: row.id, paused: !row.paused })}
        >
          {row.paused ? "Resume" : "Pause"}
        </Button>
        <Button type="button" variant="outline" size="sm" disabled={test.isPending} onClick={() => test.mutate(row.id)}>
          Send test
        </Button>
        <Button type="button" variant="ghost" size="sm" onClick={onRename}>
          Rename
        </Button>
        <Button type="button" variant="ghost" size="sm" onClick={onRemove}>
          Remove
        </Button>
      </div>
      {test.isSuccess ? (
        <Notice tone="accent" role="status">
          Test notification sent to {row.label}. If it does not arrive, check the browser&apos;s notification settings.
        </Notice>
      ) : null}
      {error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(error)}
        </Notice>
      ) : null}
    </li>
  );
}

function TurnOn({ config, onDone }: { config: PushConfig; onDone: () => void }) {
  // Rendered only once the browser facts are known (client side), so the user agent is safe to read here.
  const [label, setLabel] = useState(() => deviceLabel());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function go() {
    setBusy(true);
    setError("");
    const r = await turnOnPush(config, label);
    setBusy(false);
    if (r.ok) onDone();
    else if (r.reason === "denied") onDone();
    else setError(r.message ?? "Could not turn on notifications for this device. Try again.");
  }
  return (
    <div className="space-y-3">
      <p className="text-sm text-ink-2">
        Aurora sends a notification that an approval is waiting. It carries no request details, and you still decide in Aurora.
      </p>
      <Field id="push-label" label="Name this device">
        <input id="push-label" className={inputClass} value={label} maxLength={80} autoComplete="off" onChange={(e) => setLabel(e.target.value)} />
      </Field>
      {error ? (
        <Notice tone="danger" role="alert">
          {error}
        </Notice>
      ) : null}
      <Button type="button" disabled={busy} onClick={() => void go()}>
        {busy ? "Turning on…" : "Turn on"}
      </Button>
    </div>
  );
}

/** The Web Push card and the device list. Renders nothing while push is not available on this instance. */
export function PushDevices() {
  const config = usePushConfig();
  const available = config.data?.available === true;
  const list = usePushSubscriptions(available);
  const qc = useQueryClient();
  // Browser-only facts (support, permission, marker): null on the server.
  const envRaw = useSyncExternalStore(subscribeEnv, envSnapshot, () => "");
  const env = envRaw ? parseEnv(envRaw) : null;
  const [rename, setRename] = useState<PushSubscriptionRow | null>(null);
  const [remove, setRemove] = useState<PushSubscriptionRow | null>(null);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState("");
  const [again, setAgain] = useState(false);

  if (!available || !config.data || !env) return null;
  const rows = list.data ?? [];
  const here = rows.find((r) => r.this_session);
  const onThisDevice = !!here && env.marker;
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: pushKeys.list });
    refreshEnv();
  };

  async function confirmRemove() {
    if (!remove) return;
    setRemoving(true);
    setRemoveError("");
    try {
      await removeDevice(remove);
      setRemove(null);
      refresh();
    } catch (e) {
      setRemoveError(errorMessage(e));
    } finally {
      setRemoving(false);
    }
  }

  async function turnOnAgain() {
    setAgain(true);
    try {
      await turnOnPush(config.data as PushConfig, here?.label ?? deviceLabel());
    } finally {
      setAgain(false);
      refresh();
    }
  }

  return (
    <div className="mt-5 space-y-4 border-t border-line pt-5">
      <div>
        <h3 className="text-sm font-semibold text-ink">Notify this device when an approval needs me, even when Aurora is closed</h3>
      </div>
      {env.support !== "ok" ? (
        <Notice tone="neutral" role="status">
          {SUPPORT_TEXT[env.support]}
        </Notice>
      ) : env.denied && !onThisDevice ? (
        <Notice tone="danger" role="status">
          Notifications are blocked for this site. They can be allowed in the browser settings only.
        </Notice>
      ) : onThisDevice ? (
        <p className="text-sm text-ink-2">Notifications are on for this device.</p>
      ) : (
        <TurnOn config={config.data} onDone={refresh} />
      )}
      {list.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(list.error)}
        </Notice>
      ) : null}
      {rows.length > 0 ? (
        <ul className="divide-y divide-line" aria-label="Devices that receive approval notifications">
          {rows.map((r) => (
            <DeviceRow key={r.id} row={r} busy={again} onRename={() => setRename(r)} onRemove={() => setRemove(r)} onTurnOnAgain={() => void turnOnAgain()} />
          ))}
        </ul>
      ) : null}
      <RenameDialog target={rename} onClose={() => setRename(null)} />
      <ConfirmDialog
        open={remove !== null}
        onOpenChange={(o) => {
          if (!o) {
            setRemove(null);
            setRemoveError("");
          }
        }}
        title="Remove this device?"
        description={`${remove?.label ?? "This device"} stops getting notifications. You can turn them on again there at any time.`}
        confirmLabel="Remove"
        danger
        busy={removing}
        onConfirm={() => void confirmRemove()}
      >
        {removeError ? (
          <Notice tone="danger" role="alert">
            {removeError}
          </Notice>
        ) : null}
      </ConfirmDialog>
    </div>
  );
}
