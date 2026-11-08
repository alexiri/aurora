"use client";

import { useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { ThemeSwitch } from "@/components/theme/theme-switch";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { useUpdateMe } from "@/lib/api/account";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { useSession, type Me } from "@/lib/api/queries";

function ProfileForm({ me }: { me: Me }) {
  const update = useUpdateMe();
  const [name, setName] = useState(me.user.display_name);
  const [org, setOrg] = useState(me.default_org_id ?? "");
  const [clientErr, setClientErr] = useState("");
  const [saved, setSaved] = useState(false);
  const fe = update.error ? fieldErrors(update.error) : {};

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const trimmed = name.trim();
    if (!trimmed || trimmed.length > 80) {
      setClientErr("Display name must be 1 to 80 characters.");
      return;
    }
    setClientErr("");
    update.mutate(
      { display_name: trimmed, default_org_id: org || null },
      { onSuccess: () => setSaved(true) },
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
        <select
          id="acc-org"
          className={inputClass}
          value={org}
          onChange={(e) => setOrg(e.target.value)}
        >
          <option value="">No default</option>
          {me.memberships
            .filter((m) => m.status === "active")
            .map((m) => (
              <option key={m.org.id} value={m.org.id}>
                {m.org.name}
              </option>
            ))}
        </select>
      </Field>
      {update.error && Object.keys(fe).length === 0 ? (
        <Notice tone="danger" role="alert">
          {errorMessage(update.error)}
        </Notice>
      ) : null}
      {saved ? (
        <Notice tone="accent" role="status">
          Profile saved.
        </Notice>
      ) : null}
      <FormActions>
        <Button type="submit" size="lg" disabled={update.isPending}>
          {update.isPending ? "Saving…" : "Save changes"}
        </Button>
      </FormActions>
    </Form>
  );
}

export function ProfileView() {
  const me = useSession();
  if (me.isPending) return <Loading />;
  if (me.isError) return <ErrorState error={me.error} onRetry={() => me.refetch()} />;
  return (
    <div className="space-y-8">
      <Section id="profile" title="Profile" description="How you appear to other people in your organizations." card>
        <ProfileForm key={me.dataUpdatedAt} me={me.data} />
      </Section>
      <Section id="appearance" title="Appearance" description="Stored in this browser only." card>
        <ThemeSwitch />
      </Section>
    </div>
  );
}
