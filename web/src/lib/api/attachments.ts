"use client";

import { ApiError } from "./client";
import type { components } from "./schema";

export type Attachment = components["schemas"]["Attachment"];

export const ATTACH_MAX_PER_MESSAGE_HINT = "Too many attachments on one message. Remove some and try again.";

export function attachmentUrl(id: string): string {
  return `/v1/attachments/${encodeURIComponent(id)}/content`;
}

/** Only raster images get a thumbnail, and only via an <img> of the download URL (never SVG/HTML). */
export function canThumbnail(contentType: string | null | undefined): boolean {
  const t = (contentType ?? "").toLowerCase().split(";")[0].trim();
  return t.startsWith("image/") && t !== "image/svg+xml";
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / 1024 / 1024).toFixed(1)} MiB`;
}

/** Multipart upload (`file` field). Typed with the generated `Attachment`; the browser sets the boundary. */
export async function uploadAttachment(conversationId: string, file: File): Promise<Attachment> {
  const form = new FormData();
  form.append("file", file);
  const res = await fetch(`/v1/conversations/${encodeURIComponent(conversationId)}/attachments`, {
    method: "POST",
    credentials: "same-origin",
    body: form,
  });
  const body: unknown = await res.json().catch(() => null);
  if (!res.ok) throw ApiError.fromBody(body, res.status);
  return body as Attachment;
}

export function uploadErrorMessage(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 413 || err.code === "payload_too_large") return "That file is too large to attach (limit 10 MiB).";
    if (err.code === "too_many_attachments") return ATTACH_MAX_PER_MESSAGE_HINT;
    if (err.code === "attachment_quota_exceeded")
      return "Your organization's attachment storage is full. Ask an organization admin to free up space (Storage page) or raise the limit.";
    return err.message || "Upload failed.";
  }
  return "Upload failed. Check your connection and try again.";
}
