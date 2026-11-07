export type ThemePreference = "system" | "light" | "dark";

export const THEME_STORAGE_KEY = "aurora.theme";
export const THEME_COOKIE = "aurora_theme";

export function isPreference(v: unknown): v is ThemePreference {
  return v === "system" || v === "light" || v === "dark";
}

/**
 * I-41: constant inline script, run in <head> before first paint. It carries
 * the request's CSP nonce and has NO interpolated data. It resolves the stored
 * preference (localStorage, then cookie; default system) and sets data-theme.
 */
export const THEME_SCRIPT = `(function(){try{var p=null;try{p=localStorage.getItem("aurora.theme")}catch(e){}if(p!=="light"&&p!=="dark"&&p!=="system"){var m=document.cookie.match(/(?:^|; )aurora_theme=(light|dark|system)/);p=m?m[1]:"system"}var t=p==="system"?(window.matchMedia("(prefers-color-scheme: dark)").matches?"dark":"light"):p;document.documentElement.setAttribute("data-theme",t)}catch(e){}})();`;
