"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Section } from "@/components/ui/section";
import { useProvisionConnector } from "@/lib/api/connectors";

import { ConnectorError } from "./shared";

/** The phase 0..3 path: id and name, then a registration token shown once (13 §3). */
export function SelfHostedProvision({
  onToken,
  onCancel,
}: {
  onToken: (id: string, token: string) => void;
  onCancel: () => void;
}) {
  const provision = useProvisionConnector();
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  return (
    <Section
      id="conn-provision"
      title="Self-hosted connector (advanced)"
      description="You run the connector yourself and get a registration token to give to it."
      card
    >
      <form
        className="flex flex-wrap items-start gap-x-4 gap-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          provision.mutate(
            { id: id.trim(), display_name: name.trim() },
            {
              onSuccess: (r) => {
                onToken(r.connector.id, r.token ?? "");
                setId("");
                setName("");
              },
            },
          );
        }}
      >
        <Field id="conn-id" label="Connector id">
          <input
            id="conn-id"
            className={`${inputClass} w-56 max-w-full`}
            pattern="[a-z][a-z0-9\-]{1,31}"
            required
            value={id}
            onChange={(e) => setId(e.target.value)}
          />
        </Field>
        <Field id="conn-name" label="Display name">
          <input
            id="conn-name"
            className={`${inputClass} w-64 max-w-full`}
            maxLength={80}
            required
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </Field>
        <div className="flex flex-wrap gap-2 sm:mt-label-offset">
          <Button type="submit" disabled={provision.isPending}>
            Provision
          </Button>
          <Button type="button" variant="outline" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </form>
      <div className="mt-3 empty:hidden">
        <ConnectorError error={provision.error} />
      </div>
    </Section>
  );
}
