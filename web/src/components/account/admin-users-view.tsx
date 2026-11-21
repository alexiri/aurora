"use client";

import { useState } from "react";

import { IdpRoleBadge } from "@/components/identity/idp-role-badge";
import { IdpRolesList } from "@/components/identity/idp-roles-list";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Avatar } from "@/components/ui/avatar";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { inputClass } from "@/components/ui/field";
import { LoadMore } from "@/components/ui/load-more";
import { ErrorNotice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { useAdminUsers, usePatchAdminUser, type AdminUser } from "@/lib/api/account";
import { useSession } from "@/lib/api/queries";
import { personName } from "@/lib/workspace-name";

import { When } from "@/components/ui/when";

export function NotFoundPage() {
  return (
    <div className="max-w-page-narrow">
      <PageHeader title="404" description="This page could not be found." />
    </div>
  );
}

export function UsersTable({
  users,
  selfId,
  complete = false,
}: {
  users: AdminUser[];
  selfId?: string;
  /** The list is every user (no search, no further page): then the only instance admin cannot be removed. */
  complete?: boolean;
}) {
  const patch = usePatchAdminUser();
  const [err, setErr] = useState<{ id: string; error: unknown } | null>(null);
  const [showRoles, setShowRoles] = useState<string | null>(null);
  const admins = users.filter((x) => x.is_instance_admin && !x.disabled_at);

  function change(u: AdminUser, body: { is_instance_admin?: boolean; disabled?: boolean }) {
    setErr(null);
    patch.mutate({ id: u.id, body }, { onError: (error) => setErr({ id: u.id, error }) });
  }

  if (users.length === 0) return <EmptyState title="No users found" />;
  // (P5-REFINE-18 #17, admin-13) Role values from the sign-in are a footnote when nobody has any.
  const noRoles = users.every((u) => (u.idp_roles?.values.length ?? 0) === 0);
  return (
    <>
    <DataTable caption="Users" stackBelow={900}>
      <thead>
        <tr>
          <Th>User</Th>
          <Th>Last login</Th>
          <Th>Actions</Th>
        </tr>
      </thead>
      <tbody>
        {users.map((u) => {
          const disabled = !!u.disabled_at;
          const isSelf = u.id === selfId;
          const lastAdmin = complete && u.is_instance_admin && admins.length === 1 && admins[0]!.id === u.id;
          const canAdmin = !isSelf && !(u.is_instance_admin && lastAdmin);
          const canDisable = !isSelf;
          return (
            <tr key={u.id}>
              <Td cell="title">
                <div className="flex min-w-0 items-start gap-3">
                  <Avatar name={personName(u.display_name)} size={32} />
                  <div className="min-w-0">
                    <div className="font-medium text-ink">{personName(u.display_name)}</div>
                    {/* The e-mail's name and the role, in words: "alice · Instance admin". The full address is the tooltip. */}
                    <div className="flex flex-wrap items-center gap-x-1.5 gap-y-1 break-all text-sm text-muted" title={u.email}>
                      <span>
                        {/* A person stored by their address has no name of their own: the address is then shown once, as the address. */}
                        {u.display_name.includes("@") ? u.email : u.email.split("@")[0]} · {u.is_instance_admin ? "Instance admin" : "User"}
                      </span>
                      {u.instance_admin_idp_value ? <IdpRoleBadge value={u.instance_admin_idp_value} /> : null}
                      {disabled ? <Badge tone="danger">disabled</Badge> : null}
                    </div>
                    {u.idp_roles && showRoles === u.id ? (
                      <div className="mt-1.5 min-w-60 text-xs" data-testid="idp-role-values">
                        <IdpRolesList roles={u.idp_roles} />
                      </div>
                    ) : null}
                  </div>
                </div>
              </Td>
              <Td label="Last login" className="whitespace-nowrap text-ink-2">
                <When value={u.last_login_at} />
              </Td>
              <Td cell="actions" className="space-y-1">
                {/* (P5-POLISH-10) Your own row offers neither "Remove admin" nor "Turn off", and the last instance admin
                    cannot be removed (the server refuses both already): they are not offered. Role values live here too. */}
                {isSelf ? <p className="text-xs text-muted">This is you.</p> : null}
                {canAdmin || canDisable || u.idp_roles ? (
                  <Menu>
                    <MenuTrigger asChild>
                      <Button variant="outline" size="sm" aria-label={`Manage ${personName(u.display_name)}`}>
                        Manage
                      </Button>
                    </MenuTrigger>
                    <MenuContent align="end">
                      {u.idp_roles ? (
                        <MenuItem onSelect={() => setShowRoles(showRoles === u.id ? null : u.id)}>
                          {showRoles === u.id ? "Hide" : "Show"} role values ({u.idp_roles.values.length})
                        </MenuItem>
                      ) : null}
                      {canAdmin ? (
                        <MenuItem
                          disabled={patch.isPending || !!u.instance_admin_idp_value}
                          title={u.instance_admin_idp_value ? "Granted by an identity provider role" : undefined}
                          onSelect={() => change(u, { is_instance_admin: !u.is_instance_admin })}
                        >
                          {u.is_instance_admin ? "Remove admin" : "Make admin"}
                        </MenuItem>
                      ) : null}
                      {canDisable ? (
                        <MenuItem disabled={patch.isPending} onSelect={() => change(u, { disabled: !disabled })}>
                          {disabled ? "Turn on" : "Turn off"}
                        </MenuItem>
                      ) : null}
                    </MenuContent>
                  </Menu>
                ) : null}
                {err?.id === u.id ? (
                  <ErrorNotice error={err.error} className="max-w-xs p-2 text-xs" />
                ) : null}
              </Td>
            </tr>
          );
        })}
      </tbody>
    </DataTable>
    {noRoles ? <p className="mt-2 text-sm text-ink-2">Roles from sign-in: none</p> : null}
    </>
  );
}

export function AdminUsersView() {
  const session = useSession();
  const [input, setInput] = useState("");
  const [q, setQ] = useState("");
  const isAdmin = session.data?.user.is_instance_admin === true;
  const users = useAdminUsers(q, isAdmin);

  if (session.isPending) return <Loading />;
  if (session.isError) return <ErrorState error={session.error} onRetry={() => session.refetch()} />;
  if (!isAdmin) return <NotFoundPage />;

  return (
    <div className="max-w-page space-y-4">
      <PageHeader title="Users" description="Everyone on this instance. Admins can promote or disable accounts." className="mb-2" />
      <form
        role="search"
        className="flex max-w-md gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          setQ(input.trim());
        }}
      >
        <input
          aria-label="Search users"
          placeholder="Search by name or email"
          className={inputClass}
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
        <Button type="submit" variant="outline">
          Search
        </Button>
      </form>
      {users.isPending && users.fetchStatus !== "idle" ? <Loading /> : null}
      {users.isError ? <ErrorState error={users.error} onRetry={() => users.refetch()} /> : null}
      {users.data ? (
        <UsersTable
          users={users.data.pages.flatMap((p) => p.items)}
          selfId={session.data.user.id}
          complete={q === "" && !users.hasNextPage}
        />
      ) : null}
      <LoadMore query={users} />
    </div>
  );
}
