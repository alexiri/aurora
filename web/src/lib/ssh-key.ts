/**
 * (P5-REFINE-16, account-5) What a pasted SSH public key looks like, judged as it is typed. The server stays authoritative;
 * this only tells the person early. A private key is never sent anywhere.
 */
export type SshKeyCheck =
  | { state: "empty" }
  | { state: "ok" }
  | { state: "private"; message: string }
  | { state: "unsupported"; message: string };

export const ONLY_ED25519 = "Only ed25519 keys are supported. Create one with ssh-keygen -t ed25519.";
export const PRIVATE_KEY = "That's a private key. Paste the public key (the file ending in .pub).";

/** One key on one line: `ssh-ed25519 <base64> [comment]`. */
export function checkSshKey(text: string): SshKeyCheck {
  const t = text.trim();
  if (t === "") return { state: "empty" };
  if (t.includes("-----BEGIN")) return { state: "private", message: PRIVATE_KEY };
  if (!t.includes("\n") && /^ssh-ed25519 [A-Za-z0-9+/]{20,}={0,3}(\s.*)?$/.test(t)) return { state: "ok" };
  return { state: "unsupported", message: ONLY_ED25519 };
}
