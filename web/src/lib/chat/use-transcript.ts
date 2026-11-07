"use client";

import { useEffect, useRef, useState } from "react";

import { chatApi, type TranscriptEvent } from "@/lib/api/conversations";

import { applyEvent, emptyTranscript, loadHistory, parseStreamEvent, STREAM_EVENT_TYPES, type TranscriptState } from "./transcript";

export interface TranscriptHook {
  state: TranscriptState;
  loading: boolean;
  error: unknown;
  live: boolean;
}

/** Mount with `key={conversationId}` so state resets per conversation. History first (`/events`), then the SSE stream from the last seq; browser reconnects send Last-Event-ID. */
export function useTranscript(conversationId: string | undefined): TranscriptHook {
  const [state, setState] = useState<TranscriptState>(emptyTranscript);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<unknown>(null);
  const [live, setLive] = useState(false);
  const esRef = useRef<EventSource | null>(null);

  useEffect(() => {
    if (!conversationId) return;
    let cancelled = false;
    void (async () => {
      try {
        const hist = await loadHistory((after) => chatApi.events(conversationId, after));
        if (cancelled) return;
        setState(hist);
        setLoading(false);
        if (typeof EventSource === "undefined") return;
        const es = new EventSource(`/v1/conversations/${conversationId}/stream?after_seq=${hist.lastSeq}`);
        esRef.current = es;
        es.onopen = () => setLive(true);
        es.onerror = () => setLive(false);
        const onMsg = (e: Event) => {
          const ev: TranscriptEvent | null = parseStreamEvent((e as MessageEvent<string>).data);
          if (ev) setState((s) => applyEvent(s, ev));
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
      esRef.current?.close();
      esRef.current = null;
    };
  }, [conversationId]);

  return { state, loading, error, live };
}
