"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Section } from "@/components/ui/section";
import {
  useAdminTemplates,
  useTemplateSources,
} from "@/lib/api/agent-templates";

import { AddSourceDialog } from "./add-source-dialog";
import { SourcesTable } from "./sources-table";
import { TemplatesTable } from "./templates-table";

/**
 * Administration › What agents may do › Agent templates (C-1114): the template repositories an instance admin adds
 * by their git address, and the templates themselves — nothing is offered before a version is reviewed and approved,
 * and the previous offered version keeps working while a newer one waits.
 */
export function AdminTemplates() {
  const sources = useTemplateSources();
  const templates = useAdminTemplates();
  const [adding, setAdding] = useState(false);
  const sourceItems = sources.data?.items ?? [];
  const templateItems = templates.data?.items ?? [];
  return (
    <div className="space-y-10">
      <Section
        id="template-sources"
        title="Template repositories"
        description="Git repositories of agent templates. Aurora checks them for new versions and holds every version until you review and approve it."
        actions={
          sources.isError ? null : (
            <Button onClick={() => setAdding(true)}>Add a repository</Button>
          )
        }
      >
        {sources.isPending ? <Loading /> : null}
        {sources.isError ? <ErrorState error={sources.error} onRetry={() => sources.refetch()} /> : null}
        {sources.data && sourceItems.length === 0 ? (
          <EmptyState title="No template repositories yet" action={<Button onClick={() => setAdding(true)}>Add a repository</Button>}>
            <p className="text-sm text-ink-2">Add a git repository of agent templates; people then create agents from the templates in it.</p>
          </EmptyState>
        ) : null}
        {sourceItems.length > 0 ? <SourcesTable rows={sourceItems} caption="Template repositories of this instance" /> : null}
        <AddSourceDialog open={adding} onOpenChange={setAdding} />
      </Section>

      <Section
        id="agent-templates"
        title="Templates"
        description="What the repositories hold. A version is offered only after you review it; people keep using the offered version until you approve a newer one."
      >
        {templates.isPending ? <Loading /> : null}
        {templates.isError ? <ErrorState error={templates.error} onRetry={() => templates.refetch()} /> : null}
        {templates.data && templateItems.length === 0 ? (
          <EmptyState title="No templates yet">
            <p className="text-sm text-ink-2">Templates appear here when a repository you added holds them.</p>
          </EmptyState>
        ) : null}
        {templateItems.length > 0 ? <TemplatesTable rows={templateItems} caption="Agent templates of this instance" /> : null}
      </Section>
    </div>
  );
}
