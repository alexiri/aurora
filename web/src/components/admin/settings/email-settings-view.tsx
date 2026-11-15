"use client";

import { SettingsSectionForm, type SettingsGroup } from "@/components/admin/settings/settings-section-form";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { StatusDot } from "@/components/ui/status-dot";
import { errorMessage } from "@/lib/api/errors";
import { useSettingsSection, useTestEmail, type SettingValues } from "@/lib/api/settings";

export const EMAIL_GROUPS: SettingsGroup[] = [
  {
    title: "Mail server",
    description:
      "Where sign-in, invitation and notification e-mail is sent from. The password is sealed and write-only; a password set in the deployment's environment cannot be changed here.",
    keys: ["email.smtp_url", "email.from", "email.smtp_username", "email.smtp_password"],
  },
];

const REASON: Record<string, string> = {
  connect: "The mail server could not be reached.",
  tls: "The TLS handshake with the mail server failed.",
  auth: "The mail server refused the user name or password.",
  rejected: "The mail server rejected the message.",
  timeout: "The mail server did not answer in time.",
  log_only: "No mail server is set, so mail is only written to the log.",
};

function TestEmail({ values, dirty }: { values: SettingValues; dirty: boolean }) {
  const test = useTestEmail();
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-3">
        <Button type="button" variant="outline" disabled={test.isPending} onClick={() => test.mutate(dirty ? values : null)}>
          {test.isPending ? "Sending…" : "Send test e-mail"}
        </Button>
        <span className="text-xs text-muted">
          Sends one message to your own address{dirty ? " with the values on this page, saved or not" : " with the settings in force"}.
        </span>
      </div>
      {test.isError ? (
        <Notice tone="danger" role="alert">
          {errorMessage(test.error)}
        </Notice>
      ) : null}
      {test.data ? (
        <div role="status" aria-label="Test e-mail result" className="rounded-control border border-line p-3">
          <StatusDot tone={test.data.sent ? "accent" : "danger"}>
            {test.data.sent
              ? "The test e-mail was sent to your address."
              : `The test e-mail was not sent${test.data.error_code && REASON[test.data.error_code] ? `: ${REASON[test.data.error_code]}` : "."}`}
          </StatusDot>
        </div>
      ) : null}
    </div>
  );
}

/** Admin › Settings › Email (SET-4, C-781): the mailer with a test message. */
export function EmailSettingsView() {
  const q = useSettingsSection("email");
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const section = q.data;
  const url = section.settings.find((s) => s.key === "email.smtp_url");
  const logOnly = url === undefined || url.value === null || url.value === undefined || url.value === "";
  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        breadcrumb={[{ label: "Settings", href: "/admin/settings" }, { label: section.title }]}
        title={section.title}
        description={section.description}
      />
      {logOnly ? (
        <Notice tone="pending" role="status" className="max-w-3xl">
          No mail server is set: Aurora is in log-only mode. Sign-in and invitation links are written to the server log
          and no e-mail is sent. Set an SMTP address below to send mail.
        </Notice>
      ) : null}
      <SettingsSectionForm
        key={section.generation}
        section={section}
        groups={EMAIL_GROUPS}
        onReload={() => void q.refetch()}
        footer={(values, dirty) => <TestEmail values={values} dirty={dirty} />}
      />
    </div>
  );
}
