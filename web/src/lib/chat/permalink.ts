// (P5-CHAT-2, D-228) Message permalinks: `<page>?c=<conversation>&m=<seq>`. `c` selects the conversation (the Chat
// tab already reads it), `m` is the `seq` of a durable event to scroll to and highlight.

const CONVERSATION = "c";
const MESSAGE = "m";

/** The link Copy link puts on the clipboard: the current page with `c` and `m` set (other parameters dropped). */
export function buildPermalink(href: string, convId: string, seq: number): string {
  const url = new URL(href);
  url.search = "";
  url.hash = "";
  url.searchParams.set(CONVERSATION, convId);
  url.searchParams.set(MESSAGE, String(seq));
  return url.toString();
}

/** `m` of a query string when it is a positive integer and `c` (when present) names `convId`; else `null`. */
export function permalinkSeq(search: string, convId: string): number | null {
  const q = new URLSearchParams(search);
  const c = q.get(CONVERSATION);
  if (c && c !== convId) return null;
  const m = q.get(MESSAGE);
  if (!m || !/^[1-9]\d{0,15}$/.test(m)) return null;
  return Number(m);
}

/** The current URL without `m` (a jump is one-shot: a reload or a conversation switch must not repeat it). */
export function withoutMessageParam(href: string): string {
  const url = new URL(href);
  url.searchParams.delete(MESSAGE);
  return `${url.pathname}${url.search}${url.hash}`;
}
