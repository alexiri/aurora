"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import {
  ackConnect,
  connectFailureText,
  navigateSignIn,
  openSignInWindow,
  useConnectChannel,
  type ConnectChannelMessage,
} from "@/lib/api/apps";

export type ConnectedMessage = Extract<ConnectChannelMessage, { type: "connected" }>;

/**
 * (C-842, FIX-149) The provider's sign-in runs in a separate window while the dialog waits for its result on the
 * `BroadcastChannel`. One hook for the Reconnect dialog and the connect journey: the window is opened synchronously in
 * the click (so pop-up blockers allow it), navigated once the redirect is known, closed with the dialog, and a window
 * closed by hand reads as "Sign-in was cancelled."
 */
export function useSignInWindow(appId: string, onConnected: (m: ConnectedMessage) => void) {
  const [phase, setPhase] = useState<"form" | "waiting">("form");
  const [blocked, setBlocked] = useState(false);
  const [notice, setNotice] = useState<{ tone: "pending" | "danger"; text: string } | null>(null);
  const win = useRef<Window | null>(null);
  const url = useRef<string | null>(null);

  const close = useCallback(() => {
    win.current?.close();
    win.current = null;
  }, []);

  // The dialog going away takes the sign-in window with it; nothing waits for a result any more.
  useEffect(() => close, [close]);

  const onResult = useCallback(
    (m: ConnectChannelMessage) => {
      if (phase !== "waiting") return;
      if (m.type === "connected") {
        if (m.appId !== appId) return;
        ackConnect({ connectionId: m.connectionId });
        win.current = null;
        setPhase("form");
        onConnected(m);
      } else {
        if (m.appId !== null && m.appId !== appId) return;
        ackConnect({ error: m.error });
        win.current = null;
        setPhase("form");
        setNotice({ tone: "danger", text: connectFailureText(m.error, m.reason) });
      }
    },
    [phase, appId, onConnected],
  );
  useConnectChannel(onResult);

  // A sign-in window closed without a result: back to the form.
  useEffect(() => {
    if (phase !== "waiting") return;
    const timer = setInterval(() => {
      if (win.current?.closed) {
        win.current = null;
        setPhase("form");
        setNotice({ tone: "pending", text: "Sign-in was cancelled." });
      }
    }, 1000);
    return () => clearInterval(timer);
  }, [phase]);

  /** Open the window; call it synchronously in the click. Null when the browser blocked it. */
  const open = useCallback((): Window | null => {
    setNotice(null);
    setBlocked(false);
    const w = openSignInWindow();
    if (!w) {
      setBlocked(true);
      return null;
    }
    win.current = w;
    return w;
  }, []);

  /** The server answered with the provider's URL: send the window there and wait for the result. */
  const navigate = useCallback((w: Window, to: string) => {
    // The provider's page must not get a handle on the dashboard window (reverse tabnabbing, WEBC-3).
    navigateSignIn(w, to);
    url.current = to;
    setPhase("waiting");
  }, []);

  const reopen = useCallback(() => {
    if (win.current && !win.current.closed) {
      win.current.focus();
      return;
    }
    const w = openSignInWindow();
    if (!w || !url.current) {
      w?.close();
      setBlocked(true);
      return;
    }
    navigateSignIn(w, url.current);
    win.current = w;
  }, []);

  const cancel = useCallback(() => {
    close();
    setPhase("form");
    setNotice(null);
  }, [close]);

  return { phase, blocked, setBlocked, notice, setNotice, open, navigate, close, reopen, cancel };
}
