"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { cn } from "@/lib/utils";

const TABS = [
  { href: "/account", label: "Profile" },
  { href: "/account/security", label: "Security" },
  { href: "/account/api-keys", label: "API keys" },
  { href: "/account/delete", label: "Delete account" },
];

export function AccountNav() {
  const pathname = usePathname();
  return (
    <nav aria-label="Account" className="mb-6 flex gap-1 border-b border-neutral-200">
      {TABS.map((t) => (
        <Link
          key={t.href}
          href={t.href}
          aria-current={pathname === t.href ? "page" : undefined}
          className={cn(
            "-mb-px border-b-2 px-3 py-2 text-sm",
            pathname === t.href ? "border-neutral-900 font-medium" : "border-transparent",
          )}
        >
          {t.label}
        </Link>
      ))}
    </nav>
  );
}
