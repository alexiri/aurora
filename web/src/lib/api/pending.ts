// TODO: switch to generated client when BE-013 lands (`/v1/invitations/*`).
// Contract shapes: docs/API_CONTRACTS.md M1.
import { ApiError } from "./client";

export interface InvitationPreview {
  org: { id: string; slug: string; name: string };
  email: string;
  role: string;
  inviter_name: string | null;
  status: "pending" | "accepted" | "cancelled" | "expired";
  expires_at: string;
}

export interface AcceptedInvitation {
  org: { id: string; slug: string; name: string };
  workspace_ids: string[];
}

async function call<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: "same-origin",
    ...init,
    headers: init?.body ? { "Content-Type": "application/json" } : undefined,
  });
  const body: unknown = res.status === 204 ? null : await res.json().catch(() => null);
  if (!res.ok) throw ApiError.fromBody(body, res.status);
  return body as T;
}

export const previewInvitation = (token: string) =>
  call<InvitationPreview>(`/v1/invitations/preview?token=${encodeURIComponent(token)}`);

export const acceptInvitation = (token: string) =>
  call<AcceptedInvitation>("/v1/invitations/accept", {
    method: "POST",
    body: JSON.stringify({ token }),
  });
