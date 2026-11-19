"use client";

// Web Push in this browser (phase 5, P4-011B-4, D-134, D-266(c)(d), C-835, C-836, SEC-102).
// The service worker is registered only by a person's Turn-on gesture and unregistered when this browser's
// registration is removed; the local marker below says "this browser opted in" so a different person signing in
// never inherits the device (a 404 on re-bind clears it).

import { useEffect, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { ApiError } from "@/lib/api/client";
import {
  createPushSubscription,
  deletePushSubscription,
  fetchPushConfig,
  fetchPushSubscriptions,
  pushKeys,
  type PushConfig,
  type PushSubscriptionRow,
} from "@/lib/api/push";

export const PUSH_MARKER_KEY = "aurora.webPush";
export const PUSH_WORKER_URL = "/aurora-push-sw.js";

export type PushSupport = "ok" | "unsupported" | "insecure" | "ios_home_screen";

export function hasPushMarker(): boolean {
  try {
    return window.localStorage.getItem(PUSH_MARKER_KEY) === "1";
  } catch {
    return false;
  }
}

export function setPushMarker(on: boolean): void {
  try {
    if (on) window.localStorage.setItem(PUSH_MARKER_KEY, "1");
    else window.localStorage.removeItem(PUSH_MARKER_KEY);
  } catch {
    /* storage unavailable: the opt-in just does not persist */
  }
}

function isIos(): boolean {
  const ua = navigator.userAgent;
  return /iPad|iPhone|iPod/.test(ua) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
}

function isStandalone(): boolean {
  const nav = navigator as Navigator & { standalone?: boolean };
  return nav.standalone === true || (typeof window.matchMedia === "function" && window.matchMedia("(display-mode: standalone)").matches);
}

/** Why this browser can or cannot take push; evaluated in the browser only. */
export function pushSupport(): PushSupport {
  if (typeof window === "undefined") return "unsupported";
  // Service workers need https or localhost.
  if (!window.isSecureContext) return "insecure";
  // iOS and iPadOS: Web Push exists only for a site added to the Home Screen.
  if (isIos() && !isStandalone()) return "ios_home_screen";
  if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) return "unsupported";
  return "ok";
}

export function bytesToBase64Url(bytes: ArrayBuffer | Uint8Array): string {
  const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let s = "";
  for (const x of b) s += String.fromCharCode(x);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function base64UrlToBytes(s: string): Uint8Array<ArrayBuffer> {
  const pad = "=".repeat((4 - (s.length % 4)) % 4);
  const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/") + pad);
  const out = new Uint8Array(new ArrayBuffer(bin.length));
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Does the browser subscription use the server's active key? */
export function subscriptionUsesKey(sub: PushSubscription, publicKey: string): boolean {
  const k = sub.options?.applicationServerKey;
  return !!k && bytesToBase64Url(k) === publicKey;
}

/** "Chrome on Linux": the editable default label of a new device (never sent anywhere else). */
export function deviceLabel(ua: string = typeof navigator === "undefined" ? "" : navigator.userAgent): string {
  const browser = /Edg\//.test(ua)
    ? "Edge"
    : /OPR\//.test(ua)
      ? "Opera"
      : /Firefox\//.test(ua)
        ? "Firefox"
        : /Chrome\/|CriOS\//.test(ua)
          ? "Chrome"
          : /Safari\//.test(ua)
            ? "Safari"
            : "Browser";
  const os = /iPhone/.test(ua)
    ? "iPhone"
    : /iPad/.test(ua)
      ? "iPad"
      : /Android/.test(ua)
        ? "Android"
        : /Windows/.test(ua)
          ? "Windows"
          : /Mac OS X/.test(ua)
            ? "macOS"
            : /Linux|X11/.test(ua)
              ? "Linux"
              : "";
  return os ? `${browser} on ${os}` : browser;
}

async function currentRegistration(): Promise<ServiceWorkerRegistration | undefined> {
  return navigator.serviceWorker.getRegistration("/");
}

function body(sub: PushSubscription, vapid: string) {
  const j = sub.toJSON();
  return {
    endpoint: sub.endpoint,
    keys: { p256dh: j.keys?.p256dh ?? "", auth: j.keys?.auth ?? "" },
    expiration_time: sub.expirationTime ?? null,
    vapid_public_key: vapid,
  };
}

export type TurnOnResult =
  | { ok: true; row: PushSubscriptionRow }
  | { ok: false; reason: "denied" | "unsupported" | "unavailable" | "error"; message?: string };

/**
 * Turn on push for this browser. Call from a user gesture. Registers the worker, asks for permission, subscribes
 * with the config's key and registers the subscription; a failure after subscribing unsubscribes again.
 */
export async function turnOnPush(config: PushConfig, label: string): Promise<TurnOnResult> {
  if (pushSupport() !== "ok") return { ok: false, reason: "unsupported" };
  if (!config.available || !config.vapid_public_key) return { ok: false, reason: "unavailable" };
  let sub: PushSubscription | null = null;
  try {
    const reg = await navigator.serviceWorker.register(PUSH_WORKER_URL, { scope: "/" });
    const permission = await Notification.requestPermission();
    if (permission !== "granted") {
      await reg.unregister();
      return { ok: false, reason: "denied" };
    }
    await navigator.serviceWorker.ready;
    sub = await reg.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: base64UrlToBytes(config.vapid_public_key),
    });
    const row = await createPushSubscription({ ...body(sub, config.vapid_public_key), label: label.trim().slice(0, 80) || deviceLabel() });
    setPushMarker(true);
    return { ok: true, row };
  } catch (e) {
    try {
      await sub?.unsubscribe();
      await (await currentRegistration())?.unregister();
    } catch {
      /* best effort */
    }
    return { ok: false, reason: "error", message: e instanceof Error ? e.message : undefined };
  }
}

/** Drop this browser's subscription and registration, and the local marker. Best effort, never throws. */
export async function dropLocalPush(): Promise<void> {
  setPushMarker(false);
  try {
    if (!("serviceWorker" in navigator)) return;
    const reg = await currentRegistration();
    if (!reg) return;
    await (await reg.pushManager.getSubscription())?.unsubscribe();
    await reg.unregister();
  } catch {
    /* best effort */
  }
}

/** Remove a device: its row first; when it is this browser's, also the subscription and the worker. */
export async function removeDevice(row: PushSubscriptionRow): Promise<void> {
  await deletePushSubscription(row.id);
  if (row.this_session) await dropLocalPush();
}

export type ResyncResult = "none" | "ok" | "rotated" | "rebound" | "cleared";

/**
 * On a dashboard load with the local opt-in marker (C-836): a rotated server key → unsubscribe, re-subscribe,
 * `POST`, delete the old row; a browser the server no longer lists for this session → `POST … rebind: true`
 * (404 clears the marker, so the opt-in card shows again).
 */
export async function resyncPush(config?: PushConfig): Promise<ResyncResult> {
  if (typeof window === "undefined" || !hasPushMarker() || pushSupport() !== "ok") return "none";
  const cfg = config ?? (await fetchPushConfig());
  if (!cfg.available || !cfg.vapid_public_key) return "none";
  const key = cfg.vapid_public_key;
  const reg = await currentRegistration();
  const sub = reg ? await reg.pushManager.getSubscription() : null;
  if (!reg || !sub) {
    // The browser lost its registration: the marker is stale.
    setPushMarker(false);
    return "cleared";
  }
  const rows = await fetchPushSubscriptions();
  const mine = rows.filter((r) => r.this_session);
  if (!subscriptionUsesKey(sub, key)) {
    await sub.unsubscribe();
    const fresh = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: base64UrlToBytes(key) });
    await createPushSubscription({ ...body(fresh, key), label: mine[0]?.label ?? deviceLabel() });
    for (const r of mine) await deletePushSubscription(r.id).catch(() => undefined);
    return "rotated";
  }
  if (mine.length > 0) return "ok";
  try {
    await createPushSubscription({ ...body(sub, key), rebind: true });
    return "rebound";
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      setPushMarker(false);
      await dropLocalPush();
      return "cleared";
    }
    throw e;
  }
}

/** Sign-out hook (C-836): delete this device's row and unsubscribe before the session ends; bounded, never throws. */
export async function removeThisDevicePush(timeoutMs = 3000): Promise<void> {
  if (typeof window === "undefined" || !hasPushMarker()) return;
  const work = (async () => {
    try {
      const rows = await fetchPushSubscriptions();
      for (const r of rows.filter((x) => x.this_session)) await deletePushSubscription(r.id);
    } catch {
      /* the server-side session binding ends delivery regardless (07 §Web Push) */
    }
    await dropLocalPush();
  })();
  await Promise.race([work, new Promise<void>((resolve) => setTimeout(resolve, timeoutMs))]);
}

/** Runs the load-time re-sync once per page load; invalidates the device list when something changed. */
export function usePushResync(enabled: boolean): void {
  const qc = useQueryClient();
  const ran = useRef(false);
  useEffect(() => {
    if (!enabled || ran.current || !hasPushMarker()) return;
    ran.current = true;
    void resyncPush()
      .then((r) => {
        if (r !== "none" && r !== "ok") void qc.invalidateQueries({ queryKey: pushKeys.list });
      })
      .catch(() => undefined);
  }, [enabled, qc]);
}
