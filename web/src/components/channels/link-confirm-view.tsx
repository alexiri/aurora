"use client";

import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useEffect, useState } from "react";

import { AuthCard, Notice } from "@/components/auth/auth-card";
import { FormError } from "@/components/auth/form-error";
import { Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field } from "@/components/ui/field";
import { Select } from "@/components/ui/select";
import { ApiError } from "@/lib/api/client";
import { useChannelLinkPreview, useConfirmChannelLink, useDmDefaultOptions } from "@/lib/api/channels";
import { useSession } from "@/lib/api/queries";
import { isUnauthorized, loginUrl } from "@/lib/auth/use-unauthorized-redirect";
import { wsRoutes } from "@/lib/routes";

import { personName } from "@/lib/workspace-name";
import { channelError } from "./shared";

const PATH = "/link/channel";
const STORE = "aurora.channel-link-token";

/** Remove the single-use token from the address bar (and history entry). */
function stripToken() {
  try {
    window.history.replaceState(window.history.state, "", PATH);
  } catch {
    // ignore
  }
}

function stored(): string | null {
  try {
    return window.sessionStorage.getItem(STORE);
  } catch {
    return null;
  }
}

function remember(token: string | null) {
  try {
    if (token) window.sessionStorage.setItem(STORE, token);
    else window.sessionStorage.removeItem(STORE);
  } catch {
    // ignore: the page still works for this visit
  }
}

/**
 * The token reaches this page in the URL once; it leaves the address bar at once and is kept in
 * `sessionStorage` for the sign-in round trip (the return URL never carries it, so it does not
 * land in access logs or history before use).
 */
export function LinkConfirmView() {
  const params = useSearchParams();
  const [token] = useState(() => params.get("token") ?? stored());
  const session = useSession();
  const signedIn = session.isSuccess;
  const preview = useChannelLinkPreview(token, signedIn);
  const confirm = useConfirmChannelLink();
  // (FIX-220, C-1147) Which agent answers the person's direct messages to the shared bot. A failed load
  // just leaves the question out: the link itself never depends on it.
  const options = useDmDefaultOptions(preview.data?.integration_id, signedIn);
  // `undefined` = not touched (then a single agent is preselected); `""` = "Decide later".
  const [choice, setChoice] = useState<string | undefined>(undefined);
  const [chosenName, setChosenName] = useState<string | null>(null);

  useEffect(() => {
    if (!token) return;
    remember(token);
    stripToken();
  }, [token]);

  useEffect(() => {
    if (session.isError && isUnauthorized(session.error)) {
      // The return URL is the bare page; the token waits in sessionStorage.
      window.location.assign(loginUrl(PATH));
    }
  }, [session.isError, session.error]);

  // A used or dead token is of no further use.
  useEffect(() => {
    const dead = (e: unknown) => e instanceof ApiError && (e.code === "link_expired" || e.code === "already_linked");
    if (confirm.isSuccess || dead(confirm.error) || dead(preview.error)) remember(null);
  }, [confirm.isSuccess, confirm.error, preview.error]);

  if (!token) {
    return (
      <AuthCard title="Link your chat account">
        <FormError message="This link is incomplete. Ask for a new one in the chat." />
      </AuthCard>
    );
  }
  if (session.isPending || (session.isError && isUnauthorized(session.error))) {
    return (
      <AuthCard title="Link your chat account">
        <Loading />
      </AuthCard>
    );
  }
  if (session.isError) {
    return (
      <AuthCard title="Link your chat account">
        <FormError error={session.error} />
      </AuthCard>
    );
  }

  if (confirm.isSuccess) {
    const l = confirm.data;
    const bot = options.data?.shared_bot;
    return (
      <AuthCard title="Account linked">
        <Notice>
          {chosenName && bot ? (
            <>
              Linked. Message <strong>@{bot.handle}</strong> again and <strong>{chosenName}</strong> answers.
            </>
          ) : (
            <>
              <code>{l.platform_user_ref}</code> on {l.tenant_label} is now linked to {personName(l.user.display_name)}.
            </>
          )}
        </Notice>
        <Link href="/account/channels" className="text-sm text-link underline">
          Manage chat links
        </Link>
      </AuthCard>
    );
  }

  if (preview.isPending) {
    return (
      <AuthCard title="Link your chat account">
        <Loading />
      </AuthCard>
    );
  }
  if (preview.isError) {
    return (
      <AuthCard title="Link your chat account">
        <FormError message={channelError(preview.error)} />
      </AuthCard>
    );
  }

  const p = preview.data;
  const bot = options.data?.shared_bot ?? null;
  const items = bot ? (options.data?.items ?? []) : [];
  const selected = choice ?? (items.length === 1 ? items[0].agent_id : "");
  const picked = items.find((i) => i.agent_id === selected);
  // No agent to choose from: point to where one is created (a workspace's Agents page; `/` without a workspace).
  const home = session.data.memberships?.find((m) => m.org.id === options.data?.org_id);
  const firstWorkspace = home?.workspaces[0];
  const createHref = home && firstWorkspace ? wsRoutes(home.org.slug, firstWorkspace.slug).agents() : "/";
  const terminal =
    confirm.error instanceof ApiError &&
    (confirm.error.code === "link_expired" ||
      confirm.error.code === "already_linked" ||
      confirm.error.code === "link_reference_mismatch");

  return (
    <AuthCard title="Link your chat account">
      <p className="text-sm">
        Link this chat account to <strong>{personName(session.data.user.display_name)}</strong> ({session.data.user.email}) so
        agents know who you are in chat:
      </p>
      <dl data-testid="link-account" className="space-y-1 rounded-card border border-line bg-subtle p-3 text-sm">
        <div className="flex flex-wrap gap-x-2">
          <dt className="text-muted">Integration</dt>
          <dd>
            {p.tenant_label} <span className="text-muted">({p.connector_id})</span>
          </dd>
        </div>
        {p.platform_display_name ? (
          <div className="flex flex-wrap gap-x-2">
            <dt className="text-muted">Chat name</dt>
            <dd>{p.platform_display_name}</dd>
          </div>
        ) : null}
        <div className="flex flex-wrap gap-x-2">
          <dt className="text-muted">Chat account</dt>
          <dd>
            <code>{p.platform_user_ref}</code>
          </dd>
        </div>
        {p.platform_email_hint ? (
          <div className="flex flex-wrap gap-x-2">
            <dt className="text-muted">Email on the platform</dt>
            <dd>{p.platform_email_hint}</dd>
          </div>
        ) : null}
      </dl>
      <p className="text-sm text-muted">
        Only confirm if this is your own chat account. Whoever writes as it will act as you in Aurora.
      </p>
      {options.isPending ? <Loading /> : null}
      {bot && items.length > 0 ? (
        <Field
          id="link-agent"
          label={`Agent that answers your direct messages to @${bot.handle}`}
          hint="You can change this later on your account's chat page."
        >
          <Select
            id="link-agent"
            value={selected}
            disabled={confirm.isPending || terminal}
            onChange={setChoice}
          >
            <option value="">Decide later</option>
            {items.map((i) => (
              <option key={i.agent_id} value={i.agent_id}>
                {`${i.name} (@${i.handle}) · ${i.workspace_name}${i.attached ? "" : ` · will be added to @${bot.handle}`}`}
              </option>
            ))}
          </Select>
        </Field>
      ) : null}
      {bot && items.length === 0 ? (
        <p className="text-sm text-muted" data-testid="link-no-agent">
          You don&apos;t have an agent yet.{" "}
          <Link href={createHref} className="text-link underline">
            Create one
          </Link>
          .
        </p>
      ) : null}
      <FormError message={confirm.isError ? channelError(confirm.error) : undefined} />
      {!terminal ? (
        <Button
          size="lg"
          className="w-full"
          disabled={confirm.isPending || options.isPending}
          onClick={() => {
            setChosenName(picked?.name ?? null);
            confirm.mutate({
              token,
              platform_user_ref: p.platform_user_ref,
              ...(picked ? { agent_id: picked.agent_id } : {}),
            });
          }}
        >
          {confirm.isPending ? "Linking…" : "Confirm link"}
        </Button>
      ) : null}
    </AuthCard>
  );
}
