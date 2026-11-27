"use client";

import { createContext, useContext } from "react";

/** The message a reply is aimed at (C-725): shown in the reply bar, sent as `reply_to.seq`. */
export interface ReplyTarget {
  seq: number;
  author: string;
  excerpt: string;
}

/** What a message in the transcript can ask of the thread (P5-CHAT-2): reply, jump to another message, link to one. */
export interface ChatActions {
  /** Null where the conversation has no composer: nothing then offers to reply. */
  reply: ((target: ReplyTarget) => void) | null;
  jump: (seq: number) => void;
  link: (seq: number) => string;
}

const none: ChatActions = { reply: () => undefined, jump: () => undefined, link: () => "" };

export const ChatActionsContext = createContext<ChatActions>(none);

export function useChatActions(): ChatActions {
  return useContext(ChatActionsContext);
}

/** A message's text clipped for the reply bar and quote strip (a plain-text approximation of C-725's excerpt). */
export function excerptOf(text: string, max = 280): string {
  const t = text.trim().replace(/\n{3,}/g, "\n\n");
  if (t.length <= max) return t;
  const cut = t.slice(0, max - 1);
  const at = cut.lastIndexOf(" ");
  return `${(at > max / 2 ? cut.slice(0, at) : cut).trimEnd()}…`;
}
