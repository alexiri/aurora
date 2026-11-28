/**
 * (D-508(e)) Members do not see Manage <org>, so nothing may send them to one of its pages. True only when the session
 * says so: a member of `orgSlug` who is not an instance admin. A link is a navigation decision, never an authorization.
 */
import { useMe } from "@/lib/api/queries";

interface Membership {
  org: { slug: string };
  role?: string;
  effective_role?: string | null;
}

export function isOrgMemberOnly(orgSlug: string, instanceAdmin: boolean, memberships: Membership[] | undefined): boolean {
  if (instanceAdmin) return false;
  const m = memberships?.find((x) => x.org.slug === orgSlug);
  return (m?.effective_role ?? m?.role) === "member";
}

export function useOrgMemberOnly(orgSlug: string | undefined): boolean {
  const me = useMe().data;
  return !!orgSlug && isOrgMemberOnly(orgSlug, me?.user.is_instance_admin === true, me?.memberships);
}
