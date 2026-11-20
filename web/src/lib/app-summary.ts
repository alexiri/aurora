import type { PluginPackage } from "@/lib/api/plugins";

/** What to show when nothing can be said from the manifest: the details are in the fold below. */
export const SUMMARY_FALLBACK = "Review the details below.";

/** "api.github.com, a.test and 2 more" (the package's `reaches`), or `null` when it reaches no host. */
function reachesWords(p: Pick<PluginPackage, "reaches" | "reaches_more">): string | null {
  // An older API (or a partial cache entry) can leave `reaches` out: the page must still render, with no hosts said.
  if (!p.reaches || p.reaches.length === 0) return null;
  const more = (p.reaches_more ?? 0) > 0 ? ` and ${p.reaches_more} more` : "";
  return `${p.reaches.join(", ")}${more}`;
}

const sentence = (s: string) => {
  const t = s.trim().replace(/\s+/g, " ");
  return /[.!?]$/.test(t) ? t : `${t}.`;
};

/**
 * (P5-REFINE-18 #8, admin-7) What an app does, in up to three plain sentences built from its manifest: the hosts it reaches
 * ("Reaches api.github.com."), whether people sign in with it (`connect.exchange`), and what it is for (its own summary, else
 * what its token hook gives agents). Never a hook name, a claim or a digest. With nothing to say: "Review the details below."
 */
export function appSummary(p: Pick<PluginPackage, "reaches" | "reaches_more" | "permissions" | "summary" | "apps" | "package_id">): string[] {
  const name = p.apps[0]?.name || p.package_id;
  const hooks = p.permissions.hooks ?? [];
  const out: string[] = [];
  const reach = reachesWords(p);
  if (reach) out.push(`Reaches ${reach}.`);
  if (hooks.includes("connect.exchange")) out.push(`Signs people in with ${name}.`);
  if (p.summary?.trim()) out.push(sentence(p.summary));
  else if (hooks.includes("token.mint")) out.push(`Lets agents act in ${name} for the person who connected it.`);
  return out.length > 0 ? out.slice(0, 3) : [SUMMARY_FALLBACK];
}
