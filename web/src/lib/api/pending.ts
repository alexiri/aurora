// Invitation preview/accept on the generated client (BE-013 landed).
// Kept as a small module so the /join view's imports stay stable.
import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type InvitationPreview = components["schemas"]["InvitationPreview"];
export type AcceptedInvitation = components["schemas"]["AcceptInvitationResponse"];

export const previewInvitation = (token: string) =>
  unwrap(api.GET("/v1/invitations/preview", { params: { query: { token } } }));

export const acceptInvitation = (token: string) =>
  unwrap(api.POST("/v1/invitations/accept", { body: { token } }));
