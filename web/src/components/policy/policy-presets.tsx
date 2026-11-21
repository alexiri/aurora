"use client";

import { useState, type ReactNode } from "react";

import { ConfirmDetails, needsConfirmation, savedValues } from "@/components/admin/settings/settings-section-form";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { LockPill } from "@/components/ui/lock-pill";
import { ErrorNotice } from "@/components/ui/notice";
import { Altitude } from "@/components/ui/altitude";
import { useSettingsSaver, useSettingsSection, type SettingsPreview } from "@/lib/api/settings";
import { applyPreset, detectPreset, PRESET_IDS, PRESETS, type PresetId } from "@/lib/policy-presets";

/** "Open (2 settings set in Helm)" */
export function presetWords(state: ReturnType<typeof detectPreset>): string {
  if (state.preset === null) return `Custom · based on ${PRESETS[state.basedOn].label}`;
  const n = state.lockedApart.length;
  return n > 0 ? `${PRESETS[state.preset].label} (${n} setting${n === 1 ? "" : "s"} set in Helm)` : PRESETS[state.preset].label;
}

/**
 * (P5-REFINE-18 #2, D-410) "How much may agents do here?": Open, Balanced or Locked down above the policy fields of
 * Administration. Choosing one writes the mapped governance keys through the section's PUT (preview first; a loosening
 * asks in sentences). A value edited by hand turns the card into "Custom · based on Open"; a Helm-locked value that keeps
 * a preset from applying fully shows "(n settings set in Helm)" with a lock pill, never an error. `children` are today's
 * fields, behind **Customise** (Expert).
 */
export function PolicyPresets({ children }: { children?: ReactNode }) {
  const q = useSettingsSection("governance");
  const saver = useSettingsSaver();
  const [confirm, setConfirm] = useState<{ id: PresetId; preview: SettingsPreview } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [custom, setCustom] = useState(false);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const section = q.data;
  const state = detectPreset(section.settings);
  const open = custom || state.preset === null;

  async function apply(id: PresetId, confirmed = false) {
    setError(null);
    setBusy(true);
    try {
      const { values } = applyPreset(id, savedValues(section), section.settings);
      const body = { generation: section.generation, values };
      if (!confirmed) {
        const preview = await saver.preview(section.section, body);
        if (needsConfirmation(preview)) {
          setConfirm({ id, preview });
          return;
        }
      }
      await saver.put(section.section, body);
      setConfirm(null);
    } catch (e) {
      setError(e);
      setConfirm(null);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-4">
      <section id="presets" aria-labelledby="presets-h" className="scroll-mt-24 space-y-3 rounded-card border border-line bg-surface p-4" data-testid="policy-presets">
        <div className="flex flex-wrap items-center gap-2">
          <h2 id="presets-h" className="font-display text-base font-semibold text-ink">
            How much may agents do here?
          </h2>
          <Badge tone={state.preset === null ? "neutral" : "accent"} data-testid="preset-state">
            {presetWords(state)}
          </Badge>
          {state.lockedApart.length > 0 ? <LockPill owner="Set in Helm" href="/admin/set-in-helm" /> : null}
        </div>
        <ChoiceCards<PresetId | "">
          legend="How much agents may do"
          legendHidden
          value={state.preset ?? ""}
          disabled={busy}
          onChange={(v) => v && void apply(v)}
          options={PRESET_IDS.map((id) => ({ value: id, label: PRESETS[id].label, description: PRESETS[id].sentence, badge: id === "open" ? "Default" : undefined }))}
        />
        {error ? <ErrorNotice error={error} /> : null}
        {children ? (
          <Altitude id="policy.presets" level="setup">
            <div>
              <Button type="button" variant="ghost" size="sm" aria-expanded={open} aria-controls="policy-customise" onClick={() => setCustom(!open)}>
                Customise {open ? "▾" : "▸"}
              </Button>
            </div>
          </Altitude>
        ) : null}
      </section>
      {children && open ? <div id="policy-customise">{children}</div> : null}
      <ConfirmDialog
        open={confirm !== null}
        onOpenChange={(o) => {
          if (!o) setConfirm(null);
        }}
        title={confirm && confirm.preview.loosening.length > 0 ? "Allow more than before?" : "Save these settings?"}
        description={confirm ? `Set the policy to ${PRESETS[confirm.id].label}. This changes what people and agents are allowed to do.` : ""}
        confirmLabel="Yes, save"
        cancelLabel="Go back"
        busy={busy}
        onConfirm={() => confirm && void apply(confirm.id, true)}
      >
        {confirm ? (
          <div className="space-y-1">
            <ConfirmDetails preview={confirm.preview} fields={section.fields} title={section.title} />
          </div>
        ) : null}
      </ConfirmDialog>
    </div>
  );
}
