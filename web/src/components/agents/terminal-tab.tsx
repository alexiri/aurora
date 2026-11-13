"use client";

// The web terminal (P4-TERM, D-161, SEC-87, C-622): wterm with the Ghostty core, over the
// API's same-origin WebSocket (`GET /v1/agents/{agent}/terminal`, session cookie). The tab
// exists only for hosted agents while the instance's `terminal.enabled` is on; the pane
// renders to the DOM (native selection, browser find, screen-reader support), IME included.
// Bytes flow opaquely both ways — input as UTF-8 Binary frames (≤ 16 KiB each; Text frames
// are the `pty.resize` control only, FIX-054); the one control the dashboard sends is
// `pty.resize`
// (`{"kind":"pty.resize","pty_id":0,...}` — the browser cannot know the leg's allocated
// `pty_id`). Reconnects back off and stay visible; end states (frozen, refused, lost) render
// as notices, never a silent close. wterm never touches the clipboard itself: an application's
// OSC 52 write surfaces a "Copy to clipboard" confirmation (C-622); selection copy is the
// browser's own.
import { useEffect, useMemo, useRef, useState } from "react";

import { GhosttyCore } from "@wterm/ghostty";
import { Terminal, useTerminal } from "@wterm/react";
import "@wterm/react/css";

import { Notice } from "@/components/ui/notice";
import { useSandbox } from "@/lib/api/conversations";

// The vendored, version-named Ghostty WASM (sha256 recorded in scripts/guards/ghostty-wasm.sha256) (`web/public`, served same-origin as
// `application/wasm`; the CSP's `'wasm-unsafe-eval'` in script-src is what lets it compile —
// SEC-87: it grants no script evaluation).
const WASM_URL = "/ghostty-vt-0.5.10.wasm";

/**
 * Keystroke framing (FIX-054, `16 §W19`): terminal input rides **Binary** frames, UTF-8
 * encoded. wterm's `onData` hands us a string, and `WebSocket.send(string)` would send a
 * Text frame — the runner's terminal leg treats every Text frame except the `pty.resize`
 * control as a no-op (C-619: bytes are never parsed as text), so a Text keystroke is
 * silently dropped and the shell never sees it.
 */
const encoder = new TextEncoder();
/**
 * `TERMINAL_CHUNK` (`crates/aurora-core/src/limits.rs`): the runner closes the leg on any
 * Binary frame larger than this, so input — a clipboard paste arrives as one `onData`
 * string, however long — is sent as several ordered frames of at most one chunk each.
 */
const TERMINAL_CHUNK = 16 * 1024;

/** Sends terminal input as UTF-8 Binary frames, chunked to `TERMINAL_CHUNK`, in order. */
function sendInput(ws: WebSocket, data: string): void {
  const bytes = encoder.encode(data);
  for (let offset = 0; offset < bytes.length; offset += TERMINAL_CHUNK) {
    ws.send(bytes.subarray(offset, offset + TERMINAL_CHUNK));
  }
}

/** Close codes the browser sees (`16 §W19`, the W18 table): the only end-state signal. */
const CLOSE_COPY: Record<number, string> = {
  1000: "The terminal session ended.",
  4403: "Terminal access was revoked.",
  4404: "The sandbox's terminal is not here.",
  4409: "The sandbox's terminal session limit was reached.",
  4502: "The sandbox refused the terminal session.",
  4503: "The connection to the sandbox was lost.",
  4504: "The terminal session timed out.",
};

/**
 * Close codes a reconnect can plausibly fix (transient): everything else ends the pane. 1006 is an
 * abnormal close — a dropped connection, or the failed handshake the browser reports while a sandbox
 * wakes or the API restarts. The terminal ones (4403 revoked, 4404 gone, 4409 limit) stay final.
 */
const RETRYABLE = new Set([1006, 4502, 4503]);

type Phase =
  | { kind: "connecting" }
  | { kind: "open" }
  | { kind: "reconnecting"; attempt: number; max: number }
  | { kind: "ended"; code: number | null; message: string };

/** Backoff for a dropped socket: 1 s, 2 s, 4 s, capped at 8 s (`docs/14 §Terminal tab`). */
const backoff = (attempt: number) => Math.min(8000, 1000 * 2 ** attempt);
/** `docs/14`: a dropped socket retries "a few times" — four failed attempts then stop. */
const MAX_ATTEMPTS = 4;
/**
 * While a sandbox wakes, every handshake is a wake demand (C-621: 12 per hour per agent), so the
 * free retries are capped too — at the wake budget. A sandbox stuck `starting` ends the pane
 * with a notice instead of one handshake every 8 s forever.
 */
const MAX_WAKE_ATTEMPTS = 12;

/**
 * The Ghostty core, loaded once and shared by every (re)mount of the pane: `GhosttyCore.load`
 * compiles the vendored WASM (cached by URL in @wterm/ghostty). The core outlives sockets and
 * is disposed when the tab unmounts; the socket lifecycle re-uses it (a reconnect re-attaches
 * a fresh WTerm around the same core).
 */
function useGhosttyCore(disabled: boolean) {
  const [core, setCore] = useState<GhosttyCore | null>(null);
  useEffect(() => {
    if (disabled || core) return;
    let alive = true;
    void GhosttyCore.load({ wasmPath: WASM_URL }).then(
      (c) => {
        if (alive) setCore(c);
        else c.dispose();
      },
      () => {
        // A failed load shows the pane's error state; a retry happens on the next mount.
      },
    );
    return () => {
      alive = false;
    };
  }, [disabled, core]);
  useEffect(() => () => core?.dispose(), [core]);
  return core;
}

export function TerminalTab({ agentId, frozen }: { agentId: string; frozen: boolean }) {
  const sandbox = useSandbox(agentId);
  const { ref, write } = useTerminal();
  const [phase, setPhase] = useState<Phase>({ kind: "connecting" });
  // Bumped by the pane's Reconnect button: a final close ended the socket effect, so a fresh
  // run of it (not a fresh core) is what re-opens the session.
  const [retryNonce, setRetryNonce] = useState(0);
  // The parked-sandbox wake (C-621): the open IS the wake demand, answered 202 pre-upgrade —
  // the browser sees a failed handshake until the sandbox is `running`. The poll (5 s) drives
  // both the "Waking…" status and free retries while the wake is in flight.
  const waking = sandbox.data?.state === "starting";
  // The close handler reads it without re-subscribing the socket effect on every poll tick.
  const wakingRef = useRef(waking);
  const attemptRef = useRef(0);
  useEffect(() => {
    // The wake finished: the running sandbox gets the ordinary retry budget afresh.
    if (wakingRef.current && !waking) attemptRef.current = 0;
    wakingRef.current = waking;
  }, [waking]);
  // Set when a keystroke arrived while the socket was not open (never sent, never queued).
  const [droppedInput, setDroppedInput] = useState(false);
  // The OSC 52 clipboard-write confirmation (C-622: never automatic, always confirmed).
  const [copied, setCopied] = useState<string | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  // The latest measured grid: a reconnect re-opens at the current size, and the first
  // `pty.resize` rides the freshly opened socket.
  const sizeRef = useRef({ cols: 80, rows: 24 });
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const paused = frozen;

  const core = useGhosttyCore(paused);

  useEffect(() => {
    if (paused || core === null) {
      // No core yet, or the agent is paused: no socket. A paused agent tears its socket down.
      wsRef.current?.close(1000);
      wsRef.current = null;
      return;
    }
    let disposed = false;
    let ws: WebSocket | null = null;

    const connect = () => {
      if (disposed) return;
      setPhase(
        attemptRef.current === 0
          ? { kind: "connecting" }
          : { kind: "reconnecting", attempt: attemptRef.current, max: wakingRef.current ? MAX_WAKE_ATTEMPTS : MAX_ATTEMPTS },
      );
      ws = new WebSocket(`/v1/agents/${agentId}/terminal`);
      ws.binaryType = "arraybuffer";
      wsRef.current = ws;

      ws.onopen = () => {
        attemptRef.current = 0;
        setPhase({ kind: "open" });
        setDroppedInput(false);
        // The runner seeds the PTY at 80×24 (`16 §W19`); catch the pane up as soon as the
        // socket is live — the laid-out geometry rides `onResize` below.
        const { cols, rows } = sizeRef.current;
        if (cols !== 80 || rows !== 24) {
          ws?.send(JSON.stringify({ kind: "pty.resize", pty_id: 0, cols, rows }));
        }
      };
      ws.onmessage = (ev) => {
        // Binary frames are PTY bytes; text frames are not for the browser (the only leg-1
        // text control, `pty.resize`, is browser → runner) — ignore, never parse as text.
        if (typeof ev.data === "string") return;
        write(new Uint8Array(ev.data as ArrayBuffer));
      };
      ws.onclose = (ev) => {
        if (disposed || wsRef.current !== ws) return;
        wsRef.current = null;
        const message = CLOSE_COPY[ev.code] ?? "The terminal session ended unexpectedly.";
        // The code decides first (FIX-098): only a transient code is ever retried, whether or not
        // the close frame was clean (the API sends 4502/4503 as real close frames, so
        // `wasClean` is true for them) and whether or not a wake is in flight. 4403 (revoked),
        // 4404, 4409, 4504, a shell's 1000 end the pane with their reason, always.
        if (RETRYABLE.has(ev.code)) {
          // A wake in flight (C-621) retries while the sandbox poll flips `waking`; the answer
          // only becomes connectable once running. Both budgets are capped and shown.
          const max = wakingRef.current ? MAX_WAKE_ATTEMPTS : MAX_ATTEMPTS;
          if (attemptRef.current < max) {
            attemptRef.current += 1;
            setPhase({ kind: "reconnecting", attempt: attemptRef.current, max });
            timerRef.current = setTimeout(connect, backoff(attemptRef.current));
            return;
          }
          setPhase({
            kind: "ended",
            code: ev.code,
            message: wakingRef.current
              ? "The sandbox is still starting. Try again in a moment."
              : message,
          });
          return;
        }
        setPhase({ kind: "ended", code: ev.code, message });
      };
    };

    connect();
    return () => {
      disposed = true;
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      if (wsRef.current === ws) wsRef.current = null;
      ws?.close(1000);
    };
    // `write` is stable (`useTerminal`'s callback is `useCallback(..., [])`); `retryNonce`
    // re-runs the effect after the pane's explicit Reconnect.
  }, [agentId, core, paused, write, retryNonce]);

  const onResize = (cols: number, rows: number) => {
    sizeRef.current = { cols, rows };
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({ kind: "pty.resize", pty_id: 0, cols, rows }));
    }
  };

  const status = useMemo(() => {
    if (paused) return null;
    if (waking) return "Waking…";
    switch (phase.kind) {
      case "connecting":
        return "Connecting…";
      case "reconnecting":
        return (
          <>
            Reconnecting…<span className="text-muted"> (attempt {phase.attempt} of {phase.max})</span>
          </>
        );
      case "open":
        return null;
      case "ended":
        return null;
    }
  }, [phase, paused, waking]);

  if (paused) {
    return (
      <Notice tone="neutral" role="status">
        This agent is frozen. The terminal is unavailable while it is frozen.
      </Notice>
    );
  }
  if (core === null) {
    return (
      <Notice tone="pending" role="status">
        Loading terminal…
      </Notice>
    );
  }

  return (
    <div className="space-y-3">
      {status ? (
        <Notice tone="pending" role="status" aria-live="polite">
          {status}
        </Notice>
      ) : null}
      {phase.kind !== "open" ? (
        <p data-testid="terminal-not-connected" className="text-xs text-muted" role="status">
          Not connected{droppedInput ? " — what you typed was not sent." : " — typing is not sent until the terminal connects."}
        </p>
      ) : null}
      {phase.kind === "ended" && phase.code !== 1000 ? (
        <Notice tone="danger" role="alert">
          {phase.message}{" "}
          <button
            type="button"
            className="underline underline-offset-2"
            onClick={() => {
              attemptRef.current = 0;
              setPhase({ kind: "connecting" });
              // Force the socket effect to re-run by nudging its dependency: a fresh mount
              // of the pane is the simplest reliable reconnect after a final close.
              setRetryNonce((n) => n + 1);
            }}
          >
            Reconnect
          </button>
        </Notice>
      ) : null}
      {phase.kind === "ended" && phase.code === 1000 ? (
        <Notice tone="neutral" role="status">
          {phase.message}
        </Notice>
      ) : null}
      {copied !== null ? (
        <Notice tone="accent" role="status">
          <span className="flex items-center justify-between gap-3">
            <span className="min-w-0 truncate font-mono text-xs">{copied}</span>
            <button
              type="button"
              className="shrink-0 underline underline-offset-2"
              onClick={() => {
                void navigator.clipboard.writeText(copied).finally(() => setCopied(null));
              }}
            >
              Copy to clipboard
            </button>
          </span>
        </Notice>
      ) : null}
      <Terminal
        ref={ref}
        className="term-pane border-line"
        core={core}
        autoResize
        announceOutput
        aria-label="Agent terminal"
        onData={(data) => {
          if (wsRef.current?.readyState === WebSocket.OPEN) {
            sendInput(wsRef.current, data);
          } else {
            // Not connected: the keystroke is not sent and not queued (it could replay into a
            // different session); say so instead of dropping it silently.
            setDroppedInput(true);
          }
        }}
        onResize={onResize}
        onClipboardWrite={setCopied}
      />
      <p className="text-xs text-muted">The shell runs inside the agent&apos;s sandbox as its own user. Terminal output is never logged.</p>
    </div>
  );
}
