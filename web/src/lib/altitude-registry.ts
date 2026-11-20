import type { Altitude } from "@/lib/api/roles";

/**
 * (P5-REFINE-3, D-406) The classification table: every control that is not plain Use belongs to exactly one altitude.
 * Same ids and levels as `docs/14` §Altitudes and the brief. A row without `later` is wrapped with `<Altitude>` on a
 * page that exists today (the lint in `tests/altitude-registry.test.ts` checks both ways); `later` names the brief
 * that creates or rebuilds the control and wraps it. Where "area" is a page or place, not a promise of a file.
 */
export interface AltitudeEntry {
  altitude: Exclude<Altitude, "use"> | "use";
  area: string;
  /** A second level the same id is wrapped at, where the table gives one id two levels. */
  also?: "setup" | "expert";
  /** The brief that wraps it (the control does not exist in this form yet). */
  later?: string;
  /** Marks a rule with no hiding yet (Administration: Expert is on by default for instance admins). */
  mark?: true;
}

export const ALTITUDE_REGISTRY = {
  // Rail topics other than Overview, Agents and Approvals: hidden by `visibleTopics`, not by a wrapper.
  "rail.skills": { altitude: "setup", area: "rail", mark: true },
  "rail.apps": { altitude: "setup", area: "rail", mark: true },
  "rail.internet": { altitude: "setup", area: "rail", mark: true },
  "rail.activity": { altitude: "setup", area: "rail", mark: true },
  "rail.people": { altitude: "setup", area: "rail", mark: true },
  "rail.settings": { altitude: "setup", area: "rail", mark: true },
  "rail.platform": { altitude: "expert", area: "admin nav", mark: true },
  "agent.memory": { altitude: "setup", area: "agent pages" },
  "memory.key": { altitude: "expert", area: "Memory" },
  "skill.files": { altitude: "expert", area: "Skills" },
  "agent.skills": { altitude: "setup", area: "agent pages" },
  "agent.tests": { altitude: "expert", area: "agent pages" },
  "agent.otheragents": { altitude: "expert", area: "agent pages" },
  "agent.terminal": { altitude: "expert", area: "agent pages" },
  "agent.ssh": { altitude: "expert", area: "agent pages" },
  "agent.previews": { altitude: "expert", area: "agent pages" },
  "agent.connect": { altitude: "expert", area: "agent pages" },
  // The kind select is Expert; the BYO row on the Agents list is Set up (the table's "(the row: Set up)").
  "agent.byo": { altitude: "expert", also: "setup", area: "New agent, Agents list" },
  "agent.roles": { altitude: "expert", area: "Instructions" },
  "agent.effort": { altitude: "expert", area: "Instructions", mark: true },
  "agent.harness": { altitude: "expert", area: "Instructions", mark: true },
  "agent.program": { altitude: "expert", area: "Instructions", mark: true },
  "agent.proactive": { altitude: "setup", area: "Instructions" },
  "agent.handle": { altitude: "expert", area: "agent header" },
  "policy.restrict": { altitude: "setup", area: "Internet access" },
  "policy.try": { altitude: "setup", area: "Internet access" },
  "policy.presets": { altitude: "setup", area: "Internet access" },
  "auto.cron": { altitude: "expert", area: "Schedules", later: "P5-REFINE-14" },
  "auto.gate": { altitude: "expert", area: "Schedules" },
  "auto.anchor": { altitude: "expert", area: "Schedules" },
  "auto.curl": { altitude: "expert", area: "Webhooks", later: "P5-REFINE-14" },
  "auto.signature": { altitude: "expert", area: "Webhooks", later: "P5-REFINE-19" },
  "auto.deliveryids": { altitude: "expert", area: "Webhooks" },
  "connect.othermethods": { altitude: "expert", area: "Connect journey" },
  "auto.verification": { altitude: "setup", area: "Webhooks", later: "P5-REFINE-14" },
  "log.requests": { altitude: "expert", area: "Activity" },
  "tech.details": { altitude: "setup", area: "Technical details folds" },
  // (P5-REFINE-15, D-406) The Computer's own controls.
  "computer.actions": { altitude: "setup", area: "Computer (Wake it now, Restart, Sleep now)" },
  "computer.phase": { altitude: "expert", area: "Computer details (the raw phase, D-412)" },
  "files.hidden": { altitude: "expert", area: "Files (Show hidden)" },
  "terminal.window": { altitude: "expert", area: "Terminal (Open in new window)" },
  "ssh.sessions": { altitude: "expert", area: "Computer: Connect (sessions, known_hosts)" },
  "export.yaml": { altitude: "expert", area: "Settings" },
  "export.agent": { altitude: "expert", area: "Agent export and import (Settings)", later: "P5-REFINE-19" },
  // (P5-REFINE-19) Copy as curl on a webhook card and on an API key's menu.
  "copy.curl": { altitude: "expert", area: "Webhook cards, API keys", later: "P5-REFINE-19" },
  "ws.memlang": { altitude: "expert", area: "New workspace, Settings" },
  "chat.redact": { altitude: "expert", area: "Chat" },
  "org.byo": { altitude: "expert", area: "Agents (organization)" },
  "org.skills": { altitude: "expert", area: "Agents (organization)" },
  "org.groups": { altitude: "expert", area: "People" },
  "org.directory": { altitude: "expert", area: "People" },
  "apikeys": { altitude: "setup", area: "Account" },
  // The palette's verbs are a list the registry builds, not a wrapped control: `paletteVerbs` is listed when the altitude allows Expert.
  "palette.verbs": { altitude: "expert", area: "Command palette", mark: true },
  "kbd": { altitude: "expert", area: "Keyboard shortcuts" },
  "bulk": { altitude: "expert", area: "Lists", later: "P5-REFINE-19" },
  "fold.cli": { altitude: "expert", area: "CLI and API snippets" },
} as const satisfies Record<string, AltitudeEntry>;

export type AltitudeId = keyof typeof ALTITUDE_REGISTRY;

export function isAltitudeId(id: string): id is AltitudeId {
  return Object.prototype.hasOwnProperty.call(ALTITUDE_REGISTRY, id);
}
