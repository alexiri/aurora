/**
 * (FIX-138, D-246) The shell a new terminal session starts: bash (default) or zsh, remembered per browser and agent.
 * Chosen on the agent's Instructions page; read by the terminal pane (tab and standalone window) at each connect.
 */
export type Shell = "bash" | "zsh";

export const SHELL_OPTIONS: { value: Shell; label: string }[] = [
  { value: "bash", label: "bash" },
  { value: "zsh", label: "zsh" },
];

const shellKey = (agentId: string) => `aurora.terminal.shell.${agentId}`;

/** The remembered shell; bash when nothing is stored or `localStorage` is unavailable (SSR, blocked storage). */
export function readShell(agentId: string): Shell {
  try {
    return window.localStorage.getItem(shellKey(agentId)) === "zsh" ? "zsh" : "bash";
  } catch {
    return "bash";
  }
}

/** Remembers the shell for this browser; returns false when it could not be stored. */
export function writeShell(agentId: string, shell: Shell): boolean {
  try {
    window.localStorage.setItem(shellKey(agentId), shell);
    return true;
  } catch {
    return false;
  }
}
