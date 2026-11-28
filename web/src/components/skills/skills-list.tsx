"use client";

import { useSearchParams } from "next/navigation";
import { useCallback, useState } from "react";

import { useLeaveGate } from "@/components/agents/files/leave-gate";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { DiffView } from "@/components/ui/diff-view";
import { ChevronDownIcon, PlusIcon } from "@/components/ui/icons";
import { HeaderActions, ListDetail, SubScreenHeader, useNarrowScreen } from "@/components/ui/list-detail";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { ErrorNotice } from "@/components/ui/notice";
import { SaveBar, useSaveBar } from "@/components/ui/save-bar";
import { useAgentSkills,useEffectiveSkills, useOrgSkills, useSetAgentSkillOff, useSetSkillEnabled, useWorkspaceSkills } from "@/lib/api/skills";

import { SkillEditor } from "./editor";
import { AddFromLibrary, useCanAddFromLibrary } from "./add-from-library";
import { PendingUpdatesNotice } from "./library";
import { SkillChips, SkillCompactList, SkillSearch, SkillTable, SkillsFootnote, SourceChip } from "./list";
import {
  agentRows,
  emergencyOwnSkillsWords,
  levelRows,
  matchesFilter,
  matchesSearch,
  ownSkillsOnly,
  pageScope,
  pageSubject,
  pageTier,
  replaceWords,
  updateToReview,
  type SkillFilter,
  type SkillPage,
  type SkillRow,
} from "./model";
import { SkillPanel } from "./panel";
import { UploadSkillDialog } from "./upload";

/** `?skill=<id>`, written with `replace`: a reload keeps the skill on screen. */
function writeAddress(key: string | null) {
  const url = new URL(window.location.href);
  if (key) url.searchParams.set("skill", key);
  else url.searchParams.delete("skill");
  window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

type Form = { kind: "new" } | { kind: "edit"; key: string } | null;

/** The skills of a page, from the route its level has. */
function useSkillRows(page: SkillPage) {
  const agent = page.kind === "agent" ? page : null;
  // An emergency reader may read the agent's own skills, but not what it receives (that route needs the caller's own access).
  const effective = useEffectiveSkills(agent?.agentId ?? "", !!agent && !agent.readOnly);
  const own = useAgentSkills(agent?.readOnly ? agent.agentId : "");
  const workspace = useWorkspaceSkills(page.kind === "workspace" ? page.wsId : undefined);
  const organization = useOrgSkills(page.kind === "organization" ? page.orgId : undefined);
  if (page.kind === "agent" && !page.readOnly) {
    return { query: effective, rows: effective.data ? agentRows(effective.data, { orgName: page.orgName, readOnly: false }) : [] };
  }
  const query = page.kind === "agent" ? own : page.kind === "workspace" ? workspace : organization;
  const writable = page.kind === "organization" || (page.kind === "workspace" && page.canOwn);
  return { query, rows: levelRows(query.data?.items ?? [], writable) };
}

/**
 * (P5-SKILLS-3, skill-1..4) The one list of an agent's, a workspace's and an organization's skills, with the chosen
 * skill in a panel beside it (its own screen on a narrow one), the editor in the same place, and **Add skill**.
 */
export function SkillsList({ page }: { page: SkillPage }) {
  const { query, rows } = useSkillRows(page);
  const narrow = useNarrowScreen();
  const leave = useLeaveGate();
  const saved = useSaveBar();
  const params = useSearchParams();
  const scope = pageScope(page);
  const tier = pageTier(page);
  const subject = pageSubject(page);
  const canWrite = page.kind === "organization" || (page.kind === "workspace" ? page.canOwn : !page.readOnly);

  // ---- the address ----
  const routerKey = params?.get("skill") ?? null;
  const [selected, setSelected] = useState<string | null>(
    () => routerKey ?? (typeof window === "undefined" ? null : new URLSearchParams(window.location.search).get("skill")),
  );
  const [lastRouterKey, setLastRouterKey] = useState(routerKey);
  const [ownKey, setOwnKey] = useState(routerKey);
  if (routerKey !== lastRouterKey) {
    setLastRouterKey(routerKey);
    if (routerKey && routerKey !== ownKey && routerKey !== selected) setSelected(routerKey);
  }
  const select = useCallback((key: string | null) => {
    setSelected(key);
    setOwnKey(key);
    writeAddress(key);
  }, []);

  const [form, setForm] = useState<Form>(null);
  const [filter, setFilter] = useState<SkillFilter>("all");
  const [q, setQ] = useState("");
  const [comparing, setComparing] = useState<SkillRow | null>(null);
  const [library, setLibrary] = useState(false);
  const [upload, setUpload] = useState(false);
  const canAddFromLibrary = useCanAddFromLibrary(page.orgId);

  const setEnabled = useSetSkillEnabled(scope);
  const setOff = useSetAgentSkillOff(page.kind === "agent" ? page.agentId : "");

  const current = rows.find((r) => r.key === selected) ?? null;
  const editing = form?.kind === "edit" ? (rows.find((r) => r.key === form.key) ?? null) : null;
  const creating = form?.kind === "new";
  const detailOpen = creating || editing !== null || current !== null;

  const leaveForm = useCallback(() => setForm(null), []);
  const choose = (key: string) =>
    leave.ask(() => {
      leaveForm();
      saved.clear();
      // Choosing the open skill again closes it and gives the list its whole width back.
      select(!narrow && key === selected && !form ? null : key);
    });
  const back = () =>
    leave.ask(() => {
      leaveForm();
      select(null);
    });
  const startNew = () =>
    leave.ask(() => {
      saved.clear();
      setForm({ kind: "new" });
    });
  const show = (id: string) => {
    leaveForm();
    setFilter("all");
    setQ("");
    select(id);
  };

  const toggle = (row: SkillRow, on: boolean) => {
    if (!row.skill.id) return;
    setEnabled.reset();
    setOff.reset();
    if (row.control === "own") setEnabled.mutate({ id: row.skill.id, enabled: on });
    else if (row.control === "inherited") setOff.mutate({ skillId: row.skill.id, off: !on });
  };
  const busyId = setEnabled.isPending ? setEnabled.variables?.id : setOff.isPending ? setOff.variables?.skillId : undefined;
  const busyKey = busyId ?? null;

  // The folder names in use where this page writes, and the skills kept there.
  const taken = rows.filter((r) => r.tier === tier).map((r) => r.skill.name);
  const keptHere = new Set(rows.flatMap((r) => (r.tier === tier && r.skill.id ? [r.skill.id] : [])));

  const addMenu = canWrite ? (
    <Menu>
      <MenuTrigger asChild>
        <Button type="button" className="max-md:min-h-11">
          <PlusIcon className="size-4" />
          Add skill
          <ChevronDownIcon className="size-4" />
        </Button>
      </MenuTrigger>
      <MenuContent align="end">
        <MenuItem onSelect={startNew}>Write a new skill</MenuItem>
        {canAddFromLibrary ? <MenuItem onSelect={() => leave.ask(() => setLibrary(true))}>Add from a library</MenuItem> : null}
        <MenuItem onSelect={() => leave.ask(() => setUpload(true))}>Upload a skill folder</MenuItem>
      </MenuContent>
    </Menu>
  ) : null;

  if (query.isPending) return <Loading />;
  if (query.isError) return <ErrorState error={query.error} onRetry={() => query.refetch()} />;

  const visible = rows.filter((r) => matchesFilter(r, filter) && matchesSearch(r, q));
  const empty = rows.length === 0 && !creating;
  const compact = detailOpen && !narrow;
  const listProps = { page, rows: visible, selected: narrow ? null : creating ? null : (editing?.key ?? current?.key ?? null), busyKey, onSelect: choose, onToggle: toggle };

  const listColumn = (
    <div className="min-w-0 space-y-3">
      <SkillSearch value={q} onChange={setQ} />
      {!compact || filter !== "all" ? <SkillChips page={page} rows={rows} filter={filter} onFilter={setFilter} /> : null}
      {visible.length === 0 ? (
        <p role="status" className="text-sm text-muted">
          No skills match.
        </p>
      ) : compact ? (
        <SkillCompactList {...listProps} />
      ) : (
        <SkillTable {...listProps} onCompare={setComparing} />
      )}
      {compact ? null : <SkillsFootnote page={page} rows={rows} />}
    </div>
  );

  const detail =
    creating || editing ? (
      <SkillEditor
        key={editing?.key ?? "new"}
        scope={scope}
        skill={editing?.skill}
        taken={taken}
        agentName={page.kind === "agent" ? page.agentName : null}
        // A new skill's chip says where it will be kept.
        chip={<SourceChip page={page} row={editing ?? ({ tier, skill: { source: null } } as SkillRow)} />}
        narrow={narrow}
        onSaved={(id) => {
          saved.flash();
          show(id);
        }}
        onClose={leaveForm}
        onBack={back}
      />
    ) : current ? (
      <SkillPanel
        key={current.key}
        page={page}
        row={current}
        taken={taken}
        busy={busyKey === current.key}
        narrow={narrow}
        onToggle={toggle}
        onEdit={() => {
          saved.clear();
          setForm({ kind: "edit", key: current.key });
        }}
        onBack={back}
        onRemoved={() => select(null)}
        onDuplicated={show}
      />
    ) : (
      <SubScreenHeader backLabel="Skills" onBack={back} />
    );

  return (
    <div className="min-w-0 space-y-3">
      {addMenu && !(narrow && detailOpen) ? <HeaderActions>{addMenu}</HeaderActions> : null}
      {/* Counted from this page's own rows, so it agrees with the ones marked "Update available". */}
      <PendingUpdatesNotice count={rows.filter((r) => updateToReview(page, r)).length} />
      <ErrorNotice error={setEnabled.error ?? setOff.error} />
      {page.kind === "agent" && ownSkillsOnly(page) ? <p className="text-xs text-muted">{emergencyOwnSkillsWords(page.agentName)}</p> : null}
      {empty ? (
        <>
          <EmptyState
            title={ownSkillsOnly(page) ? "No skills of its own" : "No skills yet"}
            when={
              canWrite
                ? `A skill teaches ${page.kind === "agent" ? page.agentName : "agents"} how to do one task. Use Add skill to write one or bring one in.`
                : page.kind === "workspace"
                  ? "The workspace’s owners add them."
                  : "None to read."
            }
          />
          <SkillsFootnote page={page} rows={rows} />
        </>
      ) : detailOpen ? (
        <ListDetail narrow={narrow} detailOpen={detailOpen} list={listColumn} detail={detail} />
      ) : (
        listColumn
      )}
      <AddFromLibrary orgId={page.orgId} open={library} onOpenChange={setLibrary} target={{ scope, subject, skillIds: keptHere, onAdded: show }} />
      <UploadSkillDialog
        open={upload}
        onOpenChange={setUpload}
        scope={scope}
        taken={taken}
        owner={subject}
        onCreated={show}
      />
      <Dialog
        open={comparing !== null}
        onOpenChange={(o) => {
          if (!o) setComparing(null);
        }}
      >
        <DialogContent
          title={comparing ? `Compare ${comparing.name}` : "Compare"}
          description={comparing && page.kind === "agent" ? `${replaceWords(page, comparing)}. Old lines are the replaced skill’s.` : undefined}
          size="2xl"
        >
          {comparing?.replaces ? (
            <div className="space-y-4">
              <DiffView old={comparing.replaces.skill.description} next={comparing.skill.description} label="Diff of when to use it" fileName="When to use it" />
              <DiffView old={comparing.replaces.skill.body} next={comparing.skill.body} label="Diff of the instructions" fileName="SKILL.md" />
            </div>
          ) : null}
        </DialogContent>
      </Dialog>
      {leave.dialog}
      {form ? null : <SaveBar dirty={false} saved={saved.saved} onDiscard={() => undefined} />}
    </div>
  );
}
