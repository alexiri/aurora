"use client";

import Link from "next/link";
import type { ReactNode } from "react";

import { NotFoundPage } from "@/components/account/admin-users-view";
import { ErrorState, Loading } from "@/components/states";
import { useSession } from "@/lib/api/queries";

const LINKS = [
  { href: "/admin/users", label: "Users" },
  { href: "/admin/plugins", label: "Plugins" },
  { href: "/admin/host-claims", label: "Host claims" },
  { href: "/admin/oauth-clients", label: "OAuth clients" },
  { href: "/admin/trusted-keys", label: "Trusted keys" },
];

export function AdminNav() {
  return (
    <nav aria-label="Administration" className="mb-4 flex gap-4 border-b border-neutral-200 pb-2 text-sm">
      {LINKS.map((l) => (
        <Link key={l.href} href={l.href} className="underline-offset-2 hover:underline">
          {l.label}
        </Link>
      ))}
    </nav>
  );
}

export function AdminGuard({ children }: { children: ReactNode }) {
  const session = useSession();
  if (session.isPending) return <Loading />;
  if (session.isError) return <ErrorState error={session.error} onRetry={() => session.refetch()} />;
  if (session.data.user.is_instance_admin !== true) return <NotFoundPage />;
  return <>{children}</>;
}
