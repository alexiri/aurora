/**
 * (FIX-138, D-246; FIX-218, D-459) The shell a new terminal session or SSH connection starts: bash (default) or zsh. Chosen
 * on the agent's Computer page and stored on the server per person and agent (`/v1/agents/{agent}/terminal/shell`), so the
 * web terminal and SSH both use it and it follows the person across browsers.
 */
export type Shell = "bash" | "zsh";

export const SHELL_OPTIONS: { value: Shell; label: string }[] = [
  { value: "bash", label: "bash" },
  { value: "zsh", label: "zsh" },
];

const legacyKey = (agentId: string) => `aurora.terminal.shell.${agentId}`;

/**
 * The one-time move off `localStorage` (FIX-218): returns what this browser remembered for the agent (`zsh`, or `null`
 * for bash or nothing) and always removes the old key, so it is read at most once.
 */
export function takeLegacyShell(agentId: string): Shell | null {
  try {
    const value = window.localStorage.getItem(legacyKey(agentId));
    if (value === null) return null;
    window.localStorage.removeItem(legacyKey(agentId));
    return value === "zsh" ? "zsh" : null;
  } catch {
    return null;
  }
}
