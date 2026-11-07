import type { ReactNode } from "react";

export function AuthCard({ title, children }: { title: string; children: ReactNode }) {
  return (
    <main className="mx-auto w-full max-w-sm p-8">
      <p className="mb-1 text-sm font-semibold text-neutral-500">Aurora</p>
      <h1 className="mb-6 text-2xl font-semibold">{title}</h1>
      <div className="space-y-4">{children}</div>
    </main>
  );
}

export function Notice({ children }: { children: ReactNode }) {
  return (
    <p role="status" className="rounded-md border border-neutral-200 bg-neutral-50 p-3 text-sm">
      {children}
    </p>
  );
}

export const LOG_MODE_NOTICE =
  "Email delivery is not configured; the link was written to the server log.";
