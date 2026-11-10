import { Badge } from "@/components/ui/badge";

/**
 * "From IdP role `value`" (P4-IDP-1, D-139): a role the identity provider's claims give. It is
 * edited in the IdP, so the surrounding row shows it read-only.
 */
export function IdpRoleBadge({ value }: { value: string }) {
  return (
    <Badge
      tone="accent"
      title="This role comes from your identity provider and is changed there."
      className="normal-case tracking-normal"
    >
      From IdP role <code className="font-mono">{value}</code>
    </Badge>
  );
}
