"use client";

import { useEffect, useRef, useState } from "react";

import { chatApi, type TranscriptEvent } from "@/lib/api/conversations";
import { setRateNotice } from "@/lib/api/rate-limit";
import { loginUrl } from "@/lib/auth/use-unauthorized-redirect";

import { applyEvent, emptyTranscript, loadHistory, parseStreamEvent, STREAM_EVENT_TYPES, type TranscriptState } from "./transcript";

export interface TranscriptHook {
  state: TranscriptState;
  loading: boolean;
  error: unknown;
  live: boolean;
}

type Refusal = "too_many_streams" | "unauthorized" | "other";

/** Re-asks for the stream URL to learn why `EventSource` gave up (it exposes no status); aborts a granted stream at once. */
async function classifyRefusal(url: string, signal: AbortSignal): Promise<Refusal> {
  try {
    const res = await fetch(url, { headers: { Accept: "text/event-stream" }, credentials: "same-origin", signal });
    if (res.status === 401) return "unauthorized";
    if (res.status === 429) {
      const body = (await res.json().catch(() => null)) as { code?: string; error?: { code?: string } } | null;
      return (body?.code ?? body?.error?.code) === "too_many_streams" ? "too_many_streams" : "other";
    }
    void res.body?.cancel().catch(() => undefined);
    return "other";
  } catch {
    return "other";
  }
}

/** Mount with `key={conversationId}` so state resets per conversation. History first (`/events`), then the SSE stream from the last seq; browser reconnects send Last-Event-ID. */
export function useTranscript(conversationId: string | undefined): TranscriptHook {
  const [state, setState] = useState<TranscriptState>(emptyTranscript);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<unknown>(null);
  const [live, setLive] = useState(false);
  const esRef = useRef<EventSource | null>(null);
  // Bumped to reopen history + stream after a refusal that was not `too_many_streams`.
  const [nonce, setNonce] = useState(0);
  const retries = useRef(0);

  useEffect(() => {
    if (!conversationId) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const probe = new AbortController();
    void (async () => {
      try {
        const hist = await loadHistory((after) => chatApi.events(conversationId, after));
        if (cancelled) return;
        setState(hist);
        setLoading(false);
        if (typeof EventSource === "undefined") return;
        const es = new EventSource(`/v1/conversations/${conversationId}/stream?after_seq=${hist.lastSeq}`);
        esRef.current = es;
        es.onopen = () => {
          retries.current = 0;
          setLive(true);
        };
        es.onerror = (e?: Event) => {
          // The C-501 SSE event *named* `error` also fires `onerror`; it carries `data` and is a transcript event
          // (routed by the `error` listener below). Only a real connection error (no data) clears "Live".
          if (typeof (e as MessageEvent | undefined)?.data === "string") return;
          setLive(false);
          // A refused connection is CLOSED and not retried by the browser, and `EventSource` hides
          // the status. Ask the server what it was (FIX-099): only a 429 `too_many_streams`
          // is the "too many open tabs" notice; 401 means the session ended; anything else
          // (5xx, a proxy blip) reconnects with backoff from a fresh history read.
          if (es.readyState === EventSource.CLOSED) {
            es.close();
            void classifyRefusal(es.url, probe.signal).then((verdict) => {
              if (cancelled) return;
              if (verdict === "too_many_streams") setRateNotice({ kind: "streams" });
              else if (verdict === "unauthorized") window.location.assign(loginUrl(window.location.pathname + window.location.search));
              else {
                const delay = Math.min(30_000, 1000 * 2 ** retries.current);
                retries.current += 1;
                timer = setTimeout(() => setNonce((n) => n + 1), delay);
              }
            });
          }
        };
        const onMsg = (e: Event) => {
          const ev: TranscriptEvent | null = parseStreamEvent((e as MessageEvent<string>).data);
          if (ev?.type === "event.redacted") {
            // RFE-010: another view redacted an event; re-read it so the content disappears here too.
            const seq = Number(ev.payload.seq);
            if (Number.isInteger(seq) && seq > 0) {
              void chatApi
                .events(conversationId, seq - 1)
                .then((page) => {
                  const fresh = page.items.find((i) => i.seq === seq);
                  if (fresh && !cancelled) setState((s) => applyEvent(s, fresh as TranscriptEvent));
                })
                .catch(() => undefined);
            }
          } else if (ev) setState((s) => applyEvent(s, ev));
        };
        for (const t of STREAM_EVENT_TYPES) es.addEventListener(t, onMsg);
      } catch (err) {
        if (!cancelled) {
          setError(err);
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
      probe.abort();
      if (timer !== null) clearTimeout(timer);
      esRef.current?.close();
      esRef.current = null;
    };
  }, [conversationId, nonce]);

  return { state, loading, error, live };
}
