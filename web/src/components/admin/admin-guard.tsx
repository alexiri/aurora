"use client";

import type { ReactNode } from "react";

import { NotFoundPage } from "@/components/account/admin-users-view";
import { ErrorState, Loading } from "@/components/states";
import { useSession } from "@/lib/api/queries";

export function AdminGuard({ children }: { children: ReactNode }) {
  const session = useSession();
  if (session.isPending) return <Loading />;
  if (session.isError) return <ErrorState error={session.error} onRetry={() => session.refetch()} />;
  if (session.data.user.is_instance_admin !== true) return <NotFoundPage />;
  return <>{children}</>;
}
