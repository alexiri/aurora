"use client";

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { Avatar } from "@/components/agent-avatar";
import { CreateOrgDialog } from "@/components/create-org-dialog";
import { CreateWorkspaceDialog } from "@/components/create-workspace-dialog";
import { PLACE_CARD_CLASS, PlaceText } from "@/components/nav/place-card";
import { AdminMark, hasSomewhereElse, PlaceMark, PlacePickerBody, type PickerModel, type Place } from "@/components/nav/place-picker";
import { BottomSheetContent, Dialog, DialogTrigger } from "@/components/ui/dialog";
import { ChevronDownIcon } from "@/components/ui/icons";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useInstance, useMe, useResolve, useWorkspace, useWorkspaces } from "@/lib/api/queries";
import { lastWorkspace, usePlaceRecord } from "@/lib/nav/manage-return";
import { buildPlaces, placeCount, SEARCH_ABOVE } from "@/lib/nav/places";
import { lastScope, readRecent, writeRecent } from "@/lib/nav/recent-scopes";
import { cn } from "@/lib/utils";
import { personName, workspaceDisplayName } from "@/lib/workspace-name";

const NAME = "Switch workspace or organization";

/**
 * What the picker knows about a place's surroundings: every place this person can enter (`lib/nav/places.ts`). In
 * Administration and Your account the picker is about the organization you came from (else your last one).
 */
export function usePickerModel(place: Place, open: boolean): PickerModel {
  const me = useMe();
  const viewer = me.data?.user;
  const instanceAdmin = viewer?.is_instance_admin === true;
  const memberships = me.data?.memberships;
  const record = usePlaceRecord();
  const inOrg = place.kind === "workspace" || place.kind === "org";
  const cameFrom =
    record && record.userId === viewer?.id && memberships?.some((m) => m.org.slug === record.orgSlug)
      ? record.orgSlug
      : (lastWorkspace(viewer?.id, memberships)?.orgSlug ?? memberships?.[0]?.org.slug);
  const orgSlug = inOrg ? place.orgSlug : cameFrom;
  const resolved = useResolve(orgSlug);
  const openOrg = resolved.data?.org;
  const openList = useWorkspaces(openOrg?.id);
  // The workspace on screen, as its own answer says it: an emergency reader's is a place only through it (FIX-277).
  const wsSlug = place.kind === "workspace" ? place.wsSlug : undefined;
  const onWorkspace = useResolve(orgSlug, wsSlug);
  const reading = useWorkspace(wsSlug ? onWorkspace.data?.workspace?.id : undefined).data;
  const instance = useInstance();
  const viewerId = viewer?.id;
  // Read again when the menu opens or closes, never while it is open (the order must not shuffle).
  const [seen, setSeen] = useState<{ key: string; recent: string[] }>({ key: "", recent: [] });
  const key = `${viewerId ?? ""}:${open}:${wsSlug ?? ""}`;
  if (seen.key !== key) setSeen({ key, recent: readRecent(viewerId) });
  const recent = seen.recent;
  const orgs = useMemo(
    () =>
      buildPlaces({
        viewer,
        instanceAdmin,
        memberships: memberships ?? [],
        open: orgSlug && openOrg ? { id: openOrg.id, slug: orgSlug, name: openOrg.name } : null,
        openList: openList.data?.items,
        reading,
        recent,
      }),
    [viewer, instanceAdmin, memberships, orgSlug, openOrg, openList.data, reading, recent],
  );
  const current = orgs.find((o) => o.slug === orgSlug);
  return {
    orgs,
    current,
    instanceAdmin,
    userId: viewerId,
    memberships,
    recent,
    canCreateWorkspace: current !== undefined && current.id !== undefined && current.manage,
    // (org-5) Only where sign-up lets this person create an organization; the API still refuses in words.
    canCreateOrg: me.data != null && (instanceAdmin || instance.data?.org_creation === "anyone"),
    search: placeCount(orgs, instanceAdmin) > SEARCH_ABOVE,
  };
}

interface Face {
  mark: ReactNode;
  name: string | undefined;
  kind: string | undefined;
}

/** What the place card says: where you are. */
function usePlaceFace(place: Place, model: PickerModel): Face {
  const me = useMe();
  const viewer = me.data?.user;
  const resolved = useResolve(place.orgSlug);
  const resolvedWs = useResolve(place.orgSlug, place.wsSlug).data?.workspace;
  if (place.kind === "admin") return { mark: <AdminMark />, name: "Administration", kind: "This Aurora" };
  if (place.kind === "account") {
    const name = viewer ? personName(viewer.display_name) : "Account";
    return { mark: <Avatar name={name} tintKey={viewer?.email} size={28} />, name, kind: "Your account" };
  }
  const org = model.orgs.find((o) => o.slug === place.orgSlug);
  const orgName = org?.name ?? resolved.data?.org.name ?? place.orgSlug;
  if (place.kind === "org" && place.orgSlug) {
    return { mark: <PlaceMark name={orgName!} tintKey={place.orgSlug} org />, name: orgName, kind: "Organization" };
  }
  if (place.orgSlug && place.wsSlug) {
    const own = org?.workspaces.find((w) => w.slug === place.wsSlug);
    const name = own?.label ?? (resolvedWs ? workspaceDisplayName(resolvedWs, viewer) : place.wsSlug);
    return { mark: <PlaceMark name={own?.mark ?? name} tintKey={place.wsSlug} />, name, kind: orgName };
  }
  // A page that is no place of its own (a missing one): the last workspace visited.
  const last = lastScope(viewer?.id, me.data?.memberships);
  if (!last) return { mark: null, name: "Choose a workspace", kind: undefined };
  return {
    mark: <PlaceMark name={last.wsName ?? last.orgName} tintKey={last.wsSlug ?? last.orgSlug} org={!last.wsSlug} />,
    name: last.wsName ?? last.orgName,
    kind: last.wsName ? last.orgName : "Organization",
  };
}

function usePicker(place: Place) {
  const [open, setOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [creatingOrg, setCreatingOrg] = useState(false);
  const model = usePickerModel(place, open);
  const face = usePlaceFace(place, model);
  const me = useMe();
  const search = useRef<HTMLInputElement>(null);
  const viewerId = me.data?.user.id;
  useEffect(() => {
    if (place.kind === "workspace" && place.orgSlug && place.wsSlug && viewerId) writeRecent(viewerId, `${place.orgSlug}/${place.wsSlug}`);
  }, [place.kind, place.orgSlug, place.wsSlug, viewerId]);
  const current = model.current;
  // The focus goes to the search field, else to the first row; closing returns it to the card.
  const focusFirst = (event: Event) => {
    event.preventDefault();
    const panel = event.currentTarget as HTMLElement | null;
    (search.current ?? panel?.querySelector<HTMLElement>("[data-scope-row]"))?.focus();
  };
  const body = (
    <PlacePickerBody
      model={model}
      place={place}
      onClose={() => setOpen(false)}
      onNewWorkspace={() => setCreating(true)}
      onNewOrg={() => setCreatingOrg(true)}
      searchRef={search}
    />
  );
  const dialogs = (
    <>
      {model.canCreateOrg ? <CreateOrgDialog open={creatingOrg} onOpenChange={setCreatingOrg} /> : null}
      {model.canCreateWorkspace ? (
        <CreateWorkspaceDialog orgId={current!.id!} orgSlug={current!.slug} open={creating} onOpenChange={setCreating} />
      ) : null}
    </>
  );
  return {
    open,
    setOpen,
    face,
    body,
    dialogs,
    focusFirst,
    pending: me.isPending,
    // A person with nothing else to open and nothing to make gets the card without a menu.
    plain: !me.isPending && !hasSomewhereElse(model, place),
  };
}

const cardTestId = (place: Place) => (place.kind === "admin" || place.kind === "account" ? "place-card" : undefined);

/**
 * (D-389, D-508(f)) The place card at the top of the rail: it says where you are and opens the place picker, in every
 * place.
 */
export function ScopeSwitcher({ place }: { place: Place }) {
  const p = usePicker(place);
  const card = (
    <>
      {p.face.mark}
      {p.face.mark ? " " : null}
      <PlaceText name={p.face.name} kind={p.face.kind} />
    </>
  );
  // (firstday-2) While the session loads the card is plainly not ready: muted, no hover, no chevron, busy.
  if (p.pending) {
    return (
      <div data-testid="scope-switcher" aria-busy="true" aria-disabled="true" className={cn(PLACE_CARD_CLASS, "cursor-default opacity-70")}>
        <span className="size-7 shrink-0 animate-pulse rounded-control bg-subtle motion-reduce:animate-none" aria-hidden="true" />
        <span className="min-w-0 flex-1 text-left text-sm text-muted">Loading workspaces…</span>
      </div>
    );
  }
  if (p.plain) {
    return (
      <div data-testid="scope-switcher">
        <div data-testid={cardTestId(place)} className={PLACE_CARD_CLASS}>
          {card}
        </div>
      </div>
    );
  }
  return (
    <div data-testid="scope-switcher">
      <Popover open={p.open} onOpenChange={p.setOpen}>
        <PopoverTrigger asChild>
          <button type="button" data-testid={cardTestId(place)} className={cn(PLACE_CARD_CLASS, "hover:bg-subtle")}>
            {card}
            <ChevronDownIcon className="size-4 shrink-0 text-muted" />
          </button>
        </PopoverTrigger>
        <PopoverContent
          align="start"
          sideOffset={6}
          aria-label={NAME}
          // (P5-POLISH-8, D4) Sized to the room there is; the search box stays in view while the rows scroll.
          className="max-h-[var(--radix-popover-content-available-height)] w-96 p-0"
          onOpenAutoFocus={p.focusFirst}
        >
          {p.body}
        </PopoverContent>
      </Popover>
      {p.dialogs}
    </div>
  );
}

const PILL =
  "flex min-h-11 max-w-full min-w-0 items-center gap-2 rounded-full border border-line-strong bg-surface py-1 pl-1.5 pr-3 text-left text-sm font-semibold text-ink";

/**
 * (D-508(k), nav-3d) The phone's top bar: the place card in short ("Acme · Organization"); tapping it opens the same
 * picker as a sheet from the foot of the screen.
 */
export function TopBarPlace({ place }: { place: Place }) {
  const p = usePicker(place);
  if (p.pending) {
    return <div data-testid="topbar-place" aria-busy="true" className="h-9 w-40 animate-pulse rounded-full bg-subtle motion-reduce:animate-none" />;
  }
  const words = (
    <>
      {p.face.mark}
      {p.face.mark ? " " : null}
      <span className="min-w-0 truncate">
        {p.face.name}
        {p.face.kind ? ` · ${p.face.kind}` : ""}
      </span>
    </>
  );
  if (p.plain) {
    return (
      <div data-testid="topbar-place" className="min-w-0">
        <div className={PILL}>{words}</div>
      </div>
    );
  }
  return (
    <div data-testid="topbar-place" className="min-w-0">
      <Dialog open={p.open} onOpenChange={p.setOpen}>
        <DialogTrigger asChild>
          <button type="button" className={cn(PILL, "hover:bg-subtle")}>
            {words}
            <ChevronDownIcon className="size-4 shrink-0 text-muted" />
          </button>
        </DialogTrigger>
        <BottomSheetContent title={NAME} onOpenAutoFocus={p.focusFirst}>
          {p.body}
        </BottomSheetContent>
      </Dialog>
      {p.dialogs}
    </div>
  );
}
