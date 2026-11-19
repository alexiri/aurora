"use client";

/**
 * (P5-POLISH-1, D-393) Hands the page's CSP nonce to libraries that inject a `<style>` element at runtime
 * (`react-remove-scroll-bar` behind every Radix dialog and menu reads it through `get-nonce`, which falls back to the
 * global `__webpack_nonce__`; Turbopack leaves that name alone, so a plain global is read). Without it opening a menu
 * or dialog logs a style-src violation. Set during render, before any child can open a dialog.
 */
export function CspNonce({ nonce }: { nonce?: string }) {
  if (nonce && typeof window !== "undefined") (globalThis as { __webpack_nonce__?: string }).__webpack_nonce__ = nonce;
  return null;
}
