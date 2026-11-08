import type { ReactNode } from "react";

import { AuroraLogo, AuroraMark } from "@/components/ui/aurora-logo";
import { Notice as NoticeBox } from "@/components/ui/notice";

const POINTS = [
  "Every agent has its own identity, credentials and apps.",
  "A gateway checks policy on each request and can hold it for your approval.",
  "Agents never hold real secrets.",
];

/**
 * Split auth layout (docs/14 §Visual design): dark brand panel on the left (stacked above the
 * form below 768 px), form column at most 380 px wide. Every /auth/*, /join, /link/channel and
 * /setup-error view uses it.
 */
export function AuthCard({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="grid min-h-dvh md:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
      <aside className="relative overflow-hidden bg-brand-panel px-6 py-8 text-brand-panel-ink md:flex md:flex-col md:justify-between md:px-12 md:py-12">
        <AuroraMark
          variant="dark"
          aria-hidden="true"
          className="pointer-events-none absolute -bottom-16 -right-24 w-[28rem] max-w-none opacity-[0.06]"
        />
        <div className="relative">
          <AuroraLogo variant="dark" className="h-12 w-auto" />
          <p className="mt-2 font-display text-[11px] font-bold uppercase tracking-[0.22em] text-brand-panel-muted">
            Build · Run · Collaborate
          </p>
        </div>
        <div className="relative mt-8 max-w-md md:mt-0">
          <p className="font-display text-2xl font-semibold leading-8 md:text-[28px] md:leading-9">
            Run AI agents as a team, with exactly the access you grant.
          </p>
          <ul className="mt-5 hidden space-y-3 text-sm text-brand-panel-muted md:block">
            {POINTS.map((p) => (
              <li key={p} className="flex gap-3">
                <span aria-hidden="true" className="mt-2 size-1.5 shrink-0 rounded-full bg-accent" />
                {p}
              </li>
            ))}
          </ul>
        </div>
        <span className="hidden md:block" />
      </aside>
      <main className="flex items-start justify-center bg-surface px-6 py-10 md:items-center md:py-12">
        <div className="w-full max-w-[380px]">
          <h1 className="mb-6 font-display text-2xl font-semibold text-ink">{title}</h1>
          <div className="space-y-4">{children}</div>
        </div>
      </main>
    </div>
  );
}

export function Notice({ children }: { children: ReactNode }) {
  return (
    <NoticeBox role="status">
      {children}
    </NoticeBox>
  );
}

export const LOG_MODE_NOTICE =
  "Email delivery is not configured; the link was written to the server log.";
