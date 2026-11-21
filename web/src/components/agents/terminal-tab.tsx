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
// as notices, never a silent close. An application's OSC 52 write surfaces a "Copy to clipboard"
// confirmation (C-622); selection copy is the browser's own, and the pane adds the Linux
// terminal chords and the context-menu Paste wterm lacks (FIX-156, ./terminal-clipboard.ts).
import { useParams } from "next/navigation";
import { useCallback, useEffect, useMemo, useRef, useState, type ComponentProps, type RefObject } from "react";

import { GhosttyCore } from "@wterm/ghostty";
import { Terminal, useTerminal } from "@wterm/react";
import "@wterm/react/css";

import { Button, buttonVariants } from "@/components/ui/button";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { Altitude } from "@/components/ui/altitude";
import { Notice } from "@/components/ui/notice";
import { useSandbox } from "@/lib/api/conversations";
import { formatTime } from "@/lib/format-date";

import {
  clipboardChord,
  clipboardHelp,
  copyFailedNotice,
  copyToClipboard,
  canReadClipboard,
  hasTextSelection,
  isMac,
  pasteInto,
  readClipboardText,
} from "./terminal-clipboard";
import { readShell } from "./terminal-shell";

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

/** wterm's terminal instance, as `onReady` hands it over (`@wterm/dom` is not a direct dependency). */
type WTerm = Parameters<NonNullable<ComponentProps<typeof Terminal>["onReady"]>>[0];
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

/** `a` → ^A, `[` → ESC, `@` → NUL (the control character of a printable key); anything else is sent as typed. */
export function withCtrl(data: string): string {
  if (data.length !== 1) return data;
  const c = data.toUpperCase().charCodeAt(0);
  return c >= 64 && c <= 95 ? String.fromCharCode(c - 64) : data;
}

/**
 * (P5-REFINE-15, computer-12) The key row a phone lacks: **Esc · Tab · Ctrl · ↑ · ↓ · ← →**, 44 px keys, sticky at the bottom
 * of the pane and only below 640 px. Ctrl is a latch: the next key, from here or the keyboard, goes out with Ctrl and it releases.
 * Every key writes through the pane's own input path (`sendInput`), so framing and chunking are unchanged.
 */
function PhoneKeys({ ctrl, onCtrl, onKey, arrows }: { ctrl: boolean; onCtrl: () => void; onKey: (seq: string) => void; arrows: (dir: "A" | "B" | "C" | "D") => string }) {
  const key = "inline-flex min-h-11 min-w-11 flex-1 items-center justify-center rounded-control border border-line-strong bg-surface px-2 text-sm font-medium text-ink active:bg-subtle";
  return (
    <div role="toolbar" aria-label="Terminal keys" data-testid="phone-keys" className="sticky bottom-0 z-10 flex gap-1.5 border-t border-line bg-canvas py-1.5 sm:hidden">
      <button type="button" className={key} onClick={() => onKey("\x1b")}>
        Esc
      </button>
      <button type="button" className={key} onClick={() => onKey("\t")}>
        Tab
      </button>
      <button type="button" aria-pressed={ctrl} className={`${key} ${ctrl ? "border-accent bg-accent-soft text-accent-ink" : ""}`} onClick={onCtrl}>
        Ctrl
      </button>
      <button type="button" aria-label="Up" className={key} onClick={() => onKey(arrows("A"))}>
        ↑
      </button>
      <button type="button" aria-label="Down" className={key} onClick={() => onKey(arrows("B"))}>
        ↓
      </button>
      <button type="button" aria-label="Left" className={key} onClick={() => onKey(arrows("D"))}>
        ←
      </button>
      <button type="button" aria-label="Right" className={key} onClick={() => onKey(arrows("C"))}>
        →
      </button>
    </div>
  );
}

/** How long Ctrl+Shift+V waits for the browser's own `paste` event before reading the clipboard itself (FIX-156). */
const PASTE_FALLBACK_MS = 150;

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
  | { kind: "ended"; code: number | null; message: string; /** The retries ran out: this many tries, none answered (computer-2). */ tries?: number };

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
  const exitRef = useRef<HTMLButtonElement>(null);
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        {/* (computer-1, D-416) The keep-awake is the fix; the restart line in the scrollback is the fallback. */}
        <p className="text-sm text-ink-2" data-testid="terminal-keep-awake">
          An open terminal keeps the computer awake, so a running command is never cut off.
        </p>
        {/* (computer-12) An Expert item, and not on a phone: a second window is a desktop idea. */}
        <Altitude id="terminal.window" level="expert">
          <button
            type="button"
            className={`${buttonVariants({ variant: "outline", size: "sm" })} max-sm:hidden`}
            disabled={frozen}
            onClick={() => window.open(`/o/${org}/w/${ws}/agents/${handle}/terminal`, "_blank", "noopener,noreferrer")}
          >
            Open in new window
          </button>
        </Altitude>
      </div>
      <TerminalPane agentId={agentId} frozen={frozen} exitFocusRef={exitRef} />
      {/* Where Esc, then Tab lands (the pop-out button is Expert and not on a phone): visible only while focused. */}
      <button ref={exitRef} type="button" className="sr-only focus-visible:not-sr-only focus-visible:text-xs focus-visible:underline">
        Leave the terminal (Esc, then Tab)
      </button>
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
  const wtRef = useRef<WTerm | null>(null);
  // (Owner 2026-11-14) A full-screen app (claude, vim, less, htop) is on the alternate screen, which has no scrollback.
  // wterm still renders the shell's history above it, so while it is up the pane is pinned to the app's screen and does
  // not scroll (CSS `[data-alt-screen]`); the wheel goes to the app instead (`onAltWheel`).
  const altRef = useRef(false);
  const [altScreen, setAltScreen] = useState(false);
  const follow = useCallback(() => {
    const el = paneEl.current;
    const alt = wtRef.current?.bridge?.usingAltScreen() === true;
    if (alt !== altRef.current) {
      altRef.current = alt;
      setAltScreen(alt);
      if (alt) {
        followRef.current = true;
        setScrolledUp(false);
      }
    }
    if (el && (alt || followRef.current)) el.scrollTop = el.scrollHeight;
  }, []);
  // Wheel lines not yet sent as arrow keys (trackpads send many small deltas).
  const wheelLines = useRef(0);
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
  // (computer-1, C-1058, D-416) The computer restarted: `boot_id` changed under an open session, so the running commands
  // were stopped. One muted line goes into the scrollback, once per restart, in the person's time. Never inferred from
  // anything but `boot_id`.
  const bootId = sandbox.data?.boot_id ?? null;
  const bootRef = useRef<string | null>(null);
  useEffect(() => {
    if (!bootId) return;
    if (bootRef.current === null) {
      bootRef.current = bootId;
      return;
    }
    if (bootRef.current === bootId) return;
    bootRef.current = bootId;
    write(encoder.encode(`\r\n\x1b[2m— The computer restarted at ${formatTime(Date.now())}; your running commands were stopped. —\x1b[0m\r\n`));
  }, [bootId, write]);
  // The phone key row's Ctrl latch (a ref for the keystroke handler, state for the pressed look).
  const ctrlRef = useRef(false);
  const [ctrl, setCtrl] = useState(false);
  // Set when a keystroke arrived while the socket was not open (never sent, never queued).
  const [droppedInput, setDroppedInput] = useState(false);
  // The OSC 52 clipboard-write confirmation (C-622: never automatic, always confirmed).
  const [copied, setCopied] = useState<string | null>(null);
  // The confirmation's copy failed (no clipboard API and no legacy copy): the text stays, with the way to copy it.
  const [copyFailed, setCopyFailed] = useState(false);
  // FIX-156: a Ctrl+Shift+V the browser did not turn into a paste event, and whose clipboard read was refused.
  const [pasteFailed, setPasteFailed] = useState(false);
  // The pending read for Ctrl+Shift+V (cancelled when the browser's own paste event arrives).
  const pasteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // The right-click menu: where it opened and what was selected then (opening it moves focus away from the pane).
  const [menu, setMenu] = useState<{ x: number; y: number; selected: string } | null>(null);
  const menuAnchor = useRef<HTMLSpanElement>(null);
  const mac = useMemo(() => isMac(), []);
  useEffect(
    () => () => {
      if (pasteTimer.current !== null) clearTimeout(pasteTimer.current);
    },
    [],
  );
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
      // The shell is chosen on the Computer page (D-246) and read at each connect, so a change applies to the next
      // session only. bash is the server's default, so only zsh names itself (`?shell=` is an enumeration, 422 otherwise).
      ws = new WebSocket(`/v1/agents/${agentId}/terminal${readShell(agentId) === "zsh" ? "?shell=zsh" : ""}`);
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
            tries: attemptRef.current,
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

  /**
   * The wheel over a full-screen app (capture phase on the pane's wrapper, before wterm's own handler). The pane is kept
   * on the app's screen, so wterm forwards the wheel when the app asked for mouse reports; without them the wheel moves
   * the app like the arrow keys, as Ghostty and xterm do on the alternate screen ("alternate scroll"). On the normal
   * screen nothing changes: the wheel scrolls the pane's history.
   */
  const onAltWheel = useCallback((e: WheelEvent) => {
    const core = wtRef.current?.bridge;
    const el = paneEl.current;
    if (!core || !el || !core.usingAltScreen()) return;
    el.scrollTop = el.scrollHeight;
    if ((core.mouseTracking?.() ?? 0) !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.deltaY === 0) return;
    const rows = Math.max(1, core.getRows());
    const lines = e.deltaMode === 1 ? e.deltaY : e.deltaMode === 2 ? e.deltaY * rows : e.deltaY / (el.clientHeight / rows);
    wheelLines.current += lines;
    const n = Math.trunc(wheelLines.current);
    if (n === 0) return;
    wheelLines.current -= n;
    const app = core.cursorKeysApp();
    const key = n < 0 ? (app ? "\x1bOA" : "\x1b[A") : app ? "\x1bOB" : "\x1b[B";
    const ws = wsRef.current;
    if (ws?.readyState === WebSocket.OPEN) sendInput(ws, key.repeat(Math.min(Math.abs(n), 5)));
  }, []);

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
        This agent is paused. The terminal is unavailable while it is paused.
      </Notice>
    );
  }
  if (core === null) {
    return (
      <Notice tone="neutral" role="status">
        Loading terminal…
      </Notice>
    );
  }

  const gaveUp = phase.kind === "ended" && phase.tries !== undefined;
  const retryNow = () => {
    attemptRef.current = 0;
    setPhase({ kind: "connecting" });
    // Force the socket effect to re-run by nudging its dependency: a fresh mount
    // of the pane is the simplest reliable reconnect after a final close.
    setRetryNonce((n) => n + 1);
  };

  /** wterm's hidden input: the one editable element of the pane (focus, paste and the native Paste item live there). */
  const inputEl = () => paneEl.current?.querySelector("textarea") ?? null;

  /**
   * Ctrl+Shift+V: wterm only knows `e.key === "v"` and would send ^V, so the chord is kept from it (`stopPropagation`)
   * and the browser's own paste — a `paste` event on the input — does the work through wterm's handler. A browser that
   * raises none within a moment (a context without the shortcut) is read with the async Clipboard API instead, which
   * feeds the same handler; where that is unavailable or refused the pane says so.
   */
  const armPaste = () => {
    if (pasteTimer.current !== null) clearTimeout(pasteTimer.current);
    setPasteFailed(false);
    pasteTimer.current = setTimeout(() => {
      pasteTimer.current = null;
      void readClipboardText().then((text) => {
        const ta = inputEl();
        if (text === null) setPasteFailed(true);
        else if (ta && text !== "") pasteInto(ta, text);
      });
    }, PASTE_FALLBACK_MS);
  };

  /**
   * Right-click: the browser's menu has no Paste over the (non-editable) grid, so the pane shows its own: Copy, Paste,
   * Select all. A full-screen app that asked for mouse reports keeps the right button (Shift+right-click still opens the
   * menu, as in other terminals).
   */
  const onContextMenu = (e: React.MouseEvent) => {
    const tracking = (wtRef.current?.bridge?.mouseTracking?.() ?? 0) !== 0;
    if (tracking && !e.shiftKey) return;
    e.preventDefault();
    e.stopPropagation();
    // The menu's anchor sits at the pointer: positioned through the CSSOM (the production CSP bans inline style markup).
    const anchor = menuAnchor.current;
    if (anchor) {
      anchor.style.left = `${e.clientX}px`;
      anchor.style.top = `${e.clientY}px`;
    }
    setMenu({ x: e.clientX, y: e.clientY, selected: window.getSelection()?.toString() ?? "" });
  };
  const pasteFromMenu = () => {
    setPasteFailed(false);
    void readClipboardText().then((text) => {
      const ta = inputEl();
      if (text === null) setPasteFailed(true);
      else if (ta && text !== "") pasteInto(ta, text);
    });
  };

  return (
    // (P5-REFINE-19) The terminal keeps every key, ⌘K included.
    <div className="space-y-3" data-no-shortcuts="">
      {status ? (
        <Notice tone="neutral" role="status" aria-live="polite">
          {status}
        </Notice>
      ) : null}
      {phase.kind !== "open" ? (
        <p data-testid="terminal-not-connected" className="text-xs text-muted" role="status">
          Not connected{droppedInput ? " — what you typed was not sent." : " — typing is not sent until the terminal connects."}
        </p>
      ) : null}
      {gaveUp ? (
        // (computer-2) After the last retry the pane stops: no "Reconnecting…", no cursor, no typing. This is the end state.
        <div role="alert" data-testid="terminal-gave-up" className="flex min-h-48 flex-col items-center justify-center gap-2 rounded-card border border-line bg-surface p-6 text-center">
          <p className="font-display text-base font-semibold text-ink">Couldn&apos;t connect to the computer</p>
          <p className="text-sm text-ink-2">Tried {phase.kind === "ended" ? phase.tries : 0} times. The computer didn&apos;t answer.</p>
          <Button type="button" variant="primary" size="sm" onClick={retryNow}>
            Try again
          </Button>
          <Altitude id="agent.terminal" level="expert">
            <a href="?tab=details" className="text-xs text-accent-ink underline underline-offset-2">
              Report it
            </a>
          </Altitude>
        </div>
      ) : null}
      {phase.kind === "ended" && phase.code !== 1000 && !gaveUp ? (
        <Notice tone="danger" role="alert">
          {phase.message}{" "}
          <button
            type="button"
            className="underline underline-offset-2"
            onClick={retryNow}
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
        <Notice tone={copyFailed ? "danger" : "accent"} role="status">
          <span className="flex items-center justify-between gap-3">
            <span className={copyFailed ? "min-w-0 select-text break-all font-mono text-xs" : "min-w-0 truncate font-mono text-xs"}>{copied}</span>
            <button
              type="button"
              className="shrink-0 underline underline-offset-2"
              onClick={() => {
                // The confirmation clears on success or when dismissed, never on a failed copy.
                void copyToClipboard(copied).then((ok) => {
                  if (ok) {
                    setCopyFailed(false);
                    setCopied(null);
                  } else {
                    setCopyFailed(true);
                  }
                });
              }}
            >
              Copy to clipboard
            </button>
          </span>
          {copyFailed ? <span className="mt-1 block text-xs">{copyFailedNotice(mac)}</span> : null}
        </Notice>
      ) : null}
      <div
        // The pane stays mounted (its core and scrollback) but is not shown after the last retry: no cursor, nothing to type into.
        className={gaveUp ? "relative hidden" : "relative"}
        onContextMenuCapture={onContextMenu}
        // A paste event of the browser's own (Ctrl+V, ⌘V, the menu, a Ctrl+Shift+V it handled): nothing to read.
        onPasteCapture={() => {
          if (pasteTimer.current !== null) {
            clearTimeout(pasteTimer.current);
            pasteTimer.current = null;
          }
        }}
        // Esc, then Tab leaves the terminal (the shell still gets Esc; Tab on its own is the shell's completion key).
        onKeyDownCapture={(e) => {
          const chord = clipboardChord(e);
          if (chord === "paste") {
            // Never ^V: the paste is the browser's (or the read above), through wterm's own paste path.
            e.stopPropagation();
            armPaste();
            return;
          }
          if (chord === "copy") {
            // With a selection wterm copies it; without one the Linux copy chord is not an interrupt.
            if (!hasTextSelection()) {
              e.preventDefault();
              e.stopPropagation();
            }
            return;
          }
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
          data-alt-screen={altScreen ? "" : undefined}
          onReady={(wt) => {
            wtRef.current = wt;
            paneEl.current = wt.element;
            const el = wt.element;
            // Capture on the wrapper runs before wterm's wheel handler on the pane itself.
            el.parentElement?.addEventListener("wheel", onAltWheel, { capture: true, passive: false });
            el.addEventListener(
              "scroll",
              () => {
                // A full-screen app has no history to scroll to: stay on its screen.
                if (altRef.current) {
                  if (el.scrollHeight - el.scrollTop - el.clientHeight > 1) el.scrollTop = el.scrollHeight;
                  return;
                }
                const up = el.scrollHeight - el.scrollTop - el.clientHeight > 40;
                followRef.current = !up;
                setScrolledUp(up);
              },
              { passive: true },
            );
          }}
          onData={(data) => {
            // The Ctrl latch of the phone key row applies to the next key, then releases.
            if (ctrlRef.current) {
              data = withCtrl(data);
              ctrlRef.current = false;
              setCtrl(false);
            }
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
        {scrolledUp && !altScreen ? (
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
      {phase.kind === "open" ? (
        <PhoneKeys
          ctrl={ctrl}
          onCtrl={() => {
            ctrlRef.current = !ctrlRef.current;
            setCtrl(ctrlRef.current);
          }}
          arrows={(d) => {
            const app = wtRef.current?.bridge?.cursorKeysApp() === true;
            return app ? `\x1bO${d}` : `\x1b[${d}`;
          }}
          onKey={(seq) => {
            const ws = wsRef.current;
            if (ws?.readyState !== WebSocket.OPEN) return setDroppedInput(true);
            let out = seq;
            if (ctrlRef.current) {
              ctrlRef.current = false;
              setCtrl(false);
              // Ctrl with an arrow is the modified-key sequence; any other key becomes its control character.
              out = seq.length > 2 && /^\x1b[[O]/.test(seq) ? `\x1b[1;5${seq.slice(-1)}` : withCtrl(seq);
            }
            sendInput(ws, out);
          }}
        />
      ) : null}
      <Menu open={menu !== null} onOpenChange={(open) => !open && setMenu(null)}>
        <MenuTrigger asChild>
          <span ref={menuAnchor} aria-hidden="true" className="pointer-events-none fixed size-0" />
        </MenuTrigger>
        <MenuContent
          aria-label="Terminal"
          className="min-w-40"
          // Back to the terminal, not to the invisible anchor.
          onCloseAutoFocus={(e) => {
            e.preventDefault();
            ref.current?.focus();
          }}
        >
          <MenuItem disabled={!menu?.selected} onSelect={() => void copyToClipboard(menu?.selected ?? "")}>
            Copy
          </MenuItem>
          <MenuItem
            disabled={!canReadClipboard()}
            title={canReadClipboard() ? undefined : "Paste needs a secure (https) connection; use the keyboard paste instead."}
            onSelect={pasteFromMenu}
          >
            Paste
          </MenuItem>
          <MenuItem onSelect={() => void wtRef.current?.selectAll()}>Select all</MenuItem>
        </MenuContent>
      </Menu>
      {left ? (
        <p role="status" className="text-xs text-muted">
          Left the terminal.
        </p>
      ) : null}
      {pasteFailed ? (
        <p role="status" className="text-xs text-muted">
          Could not read the clipboard — use the right-click menu&apos;s Paste.
        </p>
      ) : null}
      {/* FIX-155: the trust model, always visible (not a dialog, not dismissed): the terminal is the agent's environment. */}
      <p data-testid="terminal-sandbox-notice" className="text-xs text-muted">
        This terminal runs inside the agent&apos;s sandbox: the agent can change what runs here and can see what you type.
      </p>
      <p className="text-xs text-muted">Terminal output is never logged. {clipboardHelp(mac)}</p>
    </div>
  );
}
