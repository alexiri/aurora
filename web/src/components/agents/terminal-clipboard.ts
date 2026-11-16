// Clipboard helpers of the web terminal pane (FIX-156, C-622, docs/14 §Terminal tab). wterm's own key handler reads
// `e.key === "v"` for paste and so misses Ctrl+Shift+V (the Linux terminal shortcut), sends ^C for Ctrl+Shift+C
// without a selection, and the browser's context menu offers no Paste over the non-editable grid (the pane
// replaces it with its own Copy / Paste / Select all menu: moving wterm's input under the pointer did not make the
// browser target it on the first right-click). The pane
// (`terminal-tab.tsx`) uses these to put that right without touching wterm: a paste always goes through wterm's
// own `paste` handler (bracketed paste, ESC stripped), never straight to the socket. Clipboard contents are never
// logged, stored or put in a URL.

export type ClipboardChord = "paste" | "copy" | null;

interface Chord {
  key: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  altKey: boolean;
}

/** Ctrl+Shift+V / Ctrl+Shift+C (no Meta, no Alt): the chords wterm does not treat as clipboard keys. */
export function clipboardChord(e: Chord): ClipboardChord {
  if (!e.ctrlKey || !e.shiftKey || e.metaKey || e.altKey) return null;
  const k = e.key.toLowerCase();
  return k === "v" ? "paste" : k === "c" ? "copy" : null;
}

/** macOS (⌘ shortcuts) as the browser reports it. */
export function isMac(): boolean {
  if (typeof navigator === "undefined") return false;
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  return /mac/i.test(nav.userAgentData?.platform ?? navigator.platform ?? "");
}

/** The pane's help sentence, in the platform's own form only. */
export function clipboardHelp(mac: boolean): string {
  return mac
    ? "Copy: select, then ⌘C. Paste: ⌘V."
    : "Copy: select, then Ctrl+Shift+C. Paste: Ctrl+Shift+V.";
}

/** The failure notice of the OSC 52 confirmation (the text stays visible beside it). */
export function copyFailedNotice(mac: boolean): string {
  return `Could not copy — select the text and press ${mac ? "⌘C" : "Ctrl+Shift+C"}`;
}

/** Whether the document has selected text (wterm renders to the DOM, so its selection is the browser's). */
export function hasTextSelection(doc: Document = document): boolean {
  const sel = doc.getSelection();
  return sel !== null && sel.toString().length > 0;
}

/**
 * Copies `text`: the async Clipboard API in a secure context, else `document.execCommand("copy")` from a temporary
 * off-screen textarea (plain `http://` dev and LAN addresses have no `navigator.clipboard`). Resolves `false` when
 * neither worked; never throws.
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Denied or blocked: try the legacy path below.
  }
  const previous = document.activeElement as HTMLElement | null;
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.setAttribute("aria-hidden", "true");
  ta.style.cssText = "position:fixed;left:-9999px;top:0;opacity:0;";
  document.body.appendChild(ta);
  try {
    ta.select();
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    ta.remove();
    previous?.focus?.({ preventScroll: true });
  }
}

/** The clipboard's text, or `null` when the API is missing (insecure context) or the read was refused. */
export async function readClipboardText(): Promise<string | null> {
  try {
    if (typeof navigator === "undefined" || !navigator.clipboard?.readText) return null;
    return await navigator.clipboard.readText();
  } catch {
    return null;
  }
}

/**
 * Hands `text` to wterm as a paste: a `paste` event on its input textarea, so wterm's own handler applies bracketed
 * paste and strips ESC, and the pane's `onData` chunks it. Nothing else ever writes clipboard text to the socket.
 */
export function pasteInto(textarea: HTMLTextAreaElement, text: string): void {
  const data = new DataTransfer();
  data.setData("text/plain", text);
  textarea.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
}

/** Whether the async Clipboard API can be read here (secure contexts only): the menu's Paste needs it. */
export function canReadClipboard(): boolean {
  return typeof navigator !== "undefined" && typeof navigator.clipboard?.readText === "function";
}
