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
import { useParams } from "next/navigation";
import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";

import { GhosttyCore } from "@wterm/ghostty";
import { Terminal, useTerminal } from "@wterm/react";
import "@wterm/react/css";

import { Button, buttonVariants } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { Segmented } from "@/components/ui/segmented";
import { useSandbox } from "@/lib/api/conversations";

/** (FIX-138, D-246) The shell the next session starts: bash (default) or zsh, remembered per browser and agent. */
type Shell = "bash" | "zsh";
const shellKey = (agentId: string) => `aurora.terminal.shell.${agentId}`;
function readShell(agentId: string): Shell {
  try {
    return window.localStorage.getItem(shellKey(agentId)) === "zsh" ? "zsh" : "bash";
  } catch {
    return "bash";
  }
}

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

/** (FIX-138) The grid keeps this much history; older lines scroll away (the pane's own scrollback, never the page). */
const SCROLLBACK_LINES = 5000;
/** (FIX-138) A person who pressed Esc and then Tab within this window leaves the terminal (WCAG 2.1.2: no keyboard trap). */
const ESCAPE_CHORD_MS = 1500;

/**
 * The agent's Terminal page (docs/14 §Terminal tab): the pane plus an **Open in new window** button. The pop-out is a
 * second, independent shell on the same route and grant as the tab (FIX-138; it counts against the sandbox's leg cap
 * and the pane says so when the cap refuses it).
 */
export function TerminalTab({ agentId, frozen }: { agentId: string; frozen: boolean }) {
  const { org, ws, handle } = useParams<{ org: string; ws: string; handle: string }>();
  const popOutRef = useRef<HTMLButtonElement>(null);
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-sm text-ink-2">
          A shell inside the agent&apos;s sandbox. Tab completes in the shell; to leave the terminal with the keyboard, press Esc, then Tab.
        </p>
        <button
          ref={popOutRef}
          type="button"
          className={buttonVariants({ variant: "outline", size: "sm" })}
          disabled={frozen}
          onClick={() => window.open(`/o/${org}/w/${ws}/agents/${handle}/terminal`, "_blank", "noopener,noreferrer")}
        >
          Open in new window
        </button>
      </div>
      <TerminalPane agentId={agentId} frozen={frozen} exitFocusRef={popOutRef} />
    </div>
  );
}

/**
 * The terminal itself (used by the tab and by the standalone window route). The pane has a bounded height (CSS,
 * `.term-pane`), so `autoResize` derives a fixed grid from it and output scrolls inside the pane's scrollback instead
 * of growing the page. `variant="window"` fills the viewport.
 */
export function TerminalPane({
  agentId,
  frozen,
  variant = "tab",
  exitFocusRef,
}: {
  agentId: string;
  frozen: boolean;
  variant?: "tab" | "window";
  /** Where Esc, Tab leaves to (the control after the pane). */
  exitFocusRef?: RefObject<HTMLElement | null>;
}) {
  const sandbox = useSandbox(agentId);
  const { ref, write } = useTerminal();
  // The pane element, for "Jump to latest": set when wterm is ready.
  const paneEl = useRef<HTMLElement | null>(null);
  const [scrolledUp, setScrolledUp] = useState(false);
  // True while the view follows the cursor (the person has not scrolled up): new output and resizes re-pin the pane to
  // its bottom. wterm renders the scrollback into the pane but does not keep it pinned, so the pane does (FIX-138).
  const followRef = useRef(true);
  const follow = useCallback(() => {
    const el = paneEl.current;
    if (el && followRef.current) el.scrollTop = el.scrollHeight;
  }, []);
  const lastEscape = useRef(0);
  const [left, setLeft] = useState(false);
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
  // The shell choice (bash when localStorage is unavailable, e.g. during SSR; the pane renders only after the client-side
  // core load, so nothing of it is hydrated). Applied to the next session only — the open one keeps running, so changing
  // it never re-runs the socket effect (a ref feeds the next connect).
  const [shell, setShell] = useState<Shell>(() => readShell(agentId));
  const shellRef = useRef<Shell>(shell);
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
      // bash is the server's default, so only zsh names itself (`?shell=` is an enumeration, 422 otherwise).
      ws = new WebSocket(`/v1/agents/${agentId}/terminal${shellRef.current === "zsh" ? "?shell=zsh" : ""}`);
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
        // After wterm has laid the new output out (it renders on the next frame), keep the bottom in view.
        requestAnimationFrame(() => requestAnimationFrame(follow));
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
  }, [agentId, core, paused, write, follow, retryNonce]);

  const onResize = (cols: number, rows: number) => {
    sizeRef.current = { cols, rows };
    requestAnimationFrame(follow);
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
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Segmented<Shell>
          label="Shell"
          value={shell}
          options={[
            { value: "bash", label: "bash" },
            { value: "zsh", label: "zsh" },
          ]}
          onChange={(s) => {
            shellRef.current = s;
            setShell(s);
            try {
              window.localStorage.setItem(shellKey(agentId), s);
            } catch {
              // Not remembered; the choice still applies to the next session in this tab.
            }
          }}
        />
        <span className="text-xs text-muted">Applies to the next session (Reconnect or open a new terminal).</span>
      </div>
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
      <div
        className="relative"
        // Esc, then Tab leaves the terminal (the shell still gets Esc; Tab on its own is the shell's completion key).
        onKeyDownCapture={(e) => {
          if (e.key === "Escape") {
            lastEscape.current = Date.now();
            setLeft(false);
          } else if (e.key === "Tab" && Date.now() - lastEscape.current < ESCAPE_CHORD_MS) {
            lastEscape.current = 0;
            e.preventDefault();
            e.stopPropagation();
            exitFocusRef?.current?.focus();
            setLeft(true);
          }
        }}
      >
        <Terminal
          ref={ref}
          className={variant === "window" ? "term-pane term-pane-window border-line" : "term-pane border-line"}
          core={core}
          autoResize
          announceOutput
          scrollbackLines={SCROLLBACK_LINES}
          aria-label="Agent terminal"
          onReady={(wt) => {
            paneEl.current = wt.element;
            const el = wt.element;
            el.addEventListener(
              "scroll",
              () => {
                const up = el.scrollHeight - el.scrollTop - el.clientHeight > 40;
                followRef.current = !up;
                setScrolledUp(up);
              },
              { passive: true },
            );
          }}
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
        {scrolledUp ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="absolute bottom-3 right-4 shadow-pop"
            onClick={() => {
              const el = paneEl.current;
              followRef.current = true;
              if (el) el.scrollTop = el.scrollHeight;
              setScrolledUp(false);
              ref.current?.focus();
            }}
          >
            Jump to latest
          </Button>
        ) : null}
      </div>
      {left ? (
        <p role="status" className="text-xs text-muted">
          Left the terminal.
        </p>
      ) : null}
      <p className="text-xs text-muted">The shell runs inside the agent&apos;s sandbox as its own user. Terminal output is never logged.</p>
    </div>
  );
}
