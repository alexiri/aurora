"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { ApprovalNotifications } from "@/components/account/approval-notifications";
import { IdpRolesList } from "@/components/identity/idp-roles-list";
import { ErrorState, Loading } from "@/components/states";
import { ThemeSwitch } from "@/components/theme/theme-switch";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Form } from "@/components/ui/form";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { SaveBar, useSaveBar } from "@/components/ui/save-bar";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { SwitchField } from "@/components/ui/switch";
import { useMoreControls } from "@/lib/api/use-more-controls";
import { useUpdateMe } from "@/lib/api/account";
import { fieldErrors } from "@/lib/api/errors";
import { usePatchOnboarding } from "@/lib/api/onboarding";
import { useInstance, useSession, type Me } from "@/lib/api/queries";
import { timeZones } from "@/lib/nav/landing";
import { accountRoutes } from "@/lib/routes";

const ZONES = timeZones();

function ProfileForm({ me }: { me: Me }) {
  const update = useUpdateMe();
  const [name, setName] = useState(me.user.display_name);
  const [org, setOrg] = useState(me.default_org_id ?? "");
  const [tz, setTz] = useState(me.timezone ?? "");
  const [clientErr, setClientErr] = useState("");
  const bar = useSaveBar();
  const fe = update.error ? fieldErrors(update.error) : {};
  const dirty = name !== me.user.display_name || org !== (me.default_org_id ?? "") || tz !== (me.timezone ?? "");

  function submit(e: FormEvent) {
    e.preventDefault();
    bar.clear();
    const trimmed = name.trim();
    if (!trimmed || trimmed.length > 80) {
      setClientErr("Display name must be 1 to 80 characters.");
      return;
    }
    setClientErr("");
    update.mutate(
      { display_name: trimmed, default_org_id: org || null, timezone: tz || null },
      { onSuccess: () => bar.flash() },
    );
  }

  return (
    <Form onSubmit={submit} noValidate className="max-w-md">
      <Field id="acc-email" label="Email">
        <input id="acc-email" className={inputClass} value={me.user.email} readOnly disabled />
      </Field>
      <Field id="acc-name" label="Display name" error={clientErr || fe.display_name}>
        <input
          id="acc-name"
          className={inputClass}
          value={name}
          maxLength={80}
          onChange={(e) => setName(e.target.value)}
        />
      </Field>
      <Field
        id="acc-org"
        label="Default organization"
        hint="Where you land after signing in."
        error={fe.default_org_id}
      >
        <Select id="acc-org" value={org} placeholder="No default" onChange={setOrg} options={me.memberships
            .filter((m) => m.status === "active")
            .map((m) => ({ value: m.org.id, label: m.org.name }))} />
      </Field>
      <Field
        id="acc-tz"
        label="Time zone"
        hint="Your agents tell the time in it when they work for you. Not set: each workspace's own."
        error={fe.timezone}
      >
        <Select
          id="acc-tz"
          value={tz}
          placeholder="Not set"
          onChange={setTz}
          options={(tz === "" || ZONES.includes(tz) ? ZONES : [tz, ...ZONES]).map((z) => ({ value: z, label: z }))}
        />
      </Field>
      {update.error && Object.keys(fe).length === 0 ? (
        <ErrorNotice error={update.error} />
      ) : null}
      <SaveBar
        dirty={dirty}
        saving={update.isPending}
        saved={bar.saved}
        errorCount={Object.keys(fe).length + (clientErr ? 1 : 0)}
        onDiscard={() => {
          setName(me.user.display_name);
          setOrg(me.default_org_id ?? "");
          setTz(me.timezone ?? "");
          setClientErr("");
          update.reset();
        }}
      />
    </Form>
  );
}

export function ProfileView() {
  const me = useSession();
  const sshEnabled = useInstance().data?.features?.ssh === true;
  if (me.isPending) return <Loading />;
  if (me.isError) return <ErrorState error={me.error} onRetry={() => me.refetch()} />;
  return (
    <div className="space-y-8">
      <Section id="profile" title="Profile" description="How you appear to other people in your organizations." card>
        <ProfileForm key={me.dataUpdatedAt} me={me.data} />
      </Section>
      {(me.data.groups ?? []).length > 0 || me.data.memberships.some((m) => m.status === "active" && m.effective_role !== m.role) ? (
        <Section
          id="my-groups"
          title="Your groups and role"
          description="Groups you belong to and the organization role they give you."
        >
          <ul aria-label="Groups" className="space-y-1 text-sm">
            {me.data.memberships
              .filter((m) => m.status === "active")
              .map((m) => {
                const groups = (me.data.groups ?? []).filter((g) => g.org_id === m.org.id);
                const via = m.sources.filter((s) => s.kind === "group").map((s) => s.group?.name);
                return (
                  <li key={m.org.id}>
                    <span className="font-medium">{m.org.name}</span>: {m.effective_role ?? m.role}
                    {m.effective_role != null && m.effective_role !== m.role && via.length > 0 ? ` (via ${via.join(", ")})` : ""}
                    {groups.length > 0 ? ` — groups: ${groups.map((g) => g.name).join(", ")}` : ""}
                  </li>
                );
              })}
          </ul>
        </Section>
      ) : null}
      {me.data.idp_roles ? (
        <Section
          id="idp-roles"
          title="Roles from your sign-in"
          description="The role values your identity provider sent and what each one gives you here. Roles from the provider are changed there."
        >
          <IdpRolesList roles={me.data.idp_roles} />
        </Section>
      ) : null}
      <Section
        id="notifications"
        title="Approval notifications"
        description="How you hear about approvals that need a decision."
        card
      >
        <ApprovalNotifications me={me.data} />
      </Section>
      {sshEnabled ? (
        <Section
          id="ssh-keys"
          title="SSH keys"
          description="Register the public keys you connect with, to open a shell in a hosted agent's sandbox."
          card
        >
          <Link href={accountRoutes.ssh} className="font-medium text-accent-ink underline">
            Manage your SSH keys
          </Link>
        </Section>
      ) : null}
      <Section id="getting-started" title="Getting started" description="Replay the introduction to Aurora." card>
        <GettingStartedControls />
      </Section>
      <Section id="controls" title="Controls" card>
        <MoreControlsSwitch />
      </Section>
      <Section id="appearance" title="Appearance" description="Stored in this browser only." card>
        <ThemeSwitch />
      </Section>
    </div>
  );
}

/** (P5-REFINE-3, D-406) The per-person switch, stored server-side so it follows the person across browsers. */
function MoreControlsSwitch() {
  const { more, setMore, pending, error } = useMoreControls();
  return (
    <div className="space-y-2">
      <SwitchField
        id="acc-more-controls"
        label="Show more controls"
        hint="Shows Terminal, Tests, model settings and other expert controls. You only see what your role allows."
        checked={more}
        onChange={setMore}
        disabled={pending}
      />
      {error ? (
        <p role="alert" className="text-sm text-danger-ink">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/** "Show the tour again" and "Show the getting-started guide again" (`14 §Account`, D-141). */
function GettingStartedControls() {
  const router = useRouter();
  const patch = usePatchOnboarding();
  const [note, setNote] = useState("");
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          disabled={patch.isPending}
          onClick={() => {
            patch.mutate({ tour_completed: false });
            // The tour points at the workspace navigation: the landing page starts it.
            router.push("/?tour=1");
          }}
        >
          Show the tour again
        </Button>
        <Button
          variant="outline"
          disabled={patch.isPending}
          onClick={() => patch.mutate({ guide_dismissed: [] }, { onSuccess: () => setNote("The getting-started guide is back on your Overview.") })}
        >
          Show the getting-started guide again
        </Button>
      </div>
      {note ? (
        <Notice tone="accent" role="status">
          {note}
        </Notice>
      ) : null}
      <ErrorNotice error={patch.error} />
    </div>
  );
}
