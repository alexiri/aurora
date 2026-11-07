import { api } from "./client";
import { unwrap } from "./unwrap";

export const login = (body: { email: string; password: string }) =>
  unwrap(api.POST("/v1/auth/login", { body }));

export const signup = (body: {
  email: string;
  password: string;
  display_name: string;
  invitation_token?: string | null;
}) => unwrap(api.POST("/v1/auth/signup", { body }));

export const forgotPassword = (email: string) =>
  unwrap(api.POST("/v1/auth/password/forgot", { body: { email } }));

export const resetPassword = (body: { token: string; password: string }) =>
  unwrap(api.POST("/v1/auth/password/reset", { body }));

export const verifyEmail = (token: string) =>
  unwrap(api.POST("/v1/auth/verify-email", { body: { token } }));

export const resendVerification = () => unwrap(api.POST("/v1/auth/verify-email/resend"));

export const approveCli = (body: { user_code: string; workspace_id: string }) =>
  unwrap(api.POST("/v1/auth/cli/approve", { body }));
