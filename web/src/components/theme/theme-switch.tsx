"use client";

import { useEffect, useId, useSyncExternalStore } from "react";

import { MonitorIcon, MoonIcon, SunIcon } from "@/components/ui/icons";
import { cn } from "@/lib/utils";

import { isPreference, THEME_COOKIE, THEME_STORAGE_KEY, type ThemePreference } from "./theme";

const CHANGE_EVENT = "aurora-theme-change";

function readPreference(): ThemePreference {
  try {
    const v = localStorage.getItem(THEME_STORAGE_KEY);
    if (isPreference(v)) return v;
  } catch {
    /* storage blocked: fall through to the cookie */
  }
  const m = document.cookie.match(new RegExp(`(?:^|; )${THEME_COOKIE}=(light|dark|system)`));
  return m && isPreference(m[1]) ? m[1] : "system";
}

function systemIsDark(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function applyTheme(pref: ThemePreference) {
  const resolved = pref === "system" ? (systemIsDark() ? "dark" : "light") : pref;
  document.documentElement.setAttribute("data-theme", resolved);
}

function subscribe(cb: () => void) {
  window.addEventListener(CHANGE_EVENT, cb);
  window.addEventListener("storage", cb);
  return () => {
    window.removeEventListener(CHANGE_EVENT, cb);
    window.removeEventListener("storage", cb);
  };
}

export function useThemePreference(): [ThemePreference, (p: ThemePreference) => void] {
  const pref = useSyncExternalStore(subscribe, readPreference, () => "system" as ThemePreference);
  const set = (p: ThemePreference) => {
    try {
      localStorage.setItem(THEME_STORAGE_KEY, p);
    } catch {
      /* the cookie below still carries the choice */
    }
    const secure = window.location.protocol === "https:" ? "; Secure" : "";
    document.cookie = `${THEME_COOKIE}=${p}; Path=/; Max-Age=31536000; SameSite=Strict${secure}`;
    applyTheme(p);
    window.dispatchEvent(new Event(CHANGE_EVENT));
  };
  return [pref, set];
}

/** Mounted once in the root layout: keeps data-theme in step with the preference and the OS (I-41). */
export function ThemeSync() {
  const [pref] = useThemePreference();
  useEffect(() => {
    applyTheme(pref);
    if (pref !== "system" || typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => applyTheme("system");
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [pref]);
  return null;
}

const OPTIONS: { value: ThemePreference; label: string; Icon: typeof SunIcon }[] = [
  { value: "system", label: "Match system", Icon: MonitorIcon },
  { value: "light", label: "Light", Icon: SunIcon },
  { value: "dark", label: "Dark", Icon: MoonIcon },
];

/** System / Light / Dark radio group. Native radios give arrow-key and Space handling. */
export function ThemeSwitch({ className }: { className?: string }) {
  const [pref, setPref] = useThemePreference();
  // Unique per instance: two switches on one page must not share one native radio group.
  const name = useId();
  return (
    <div
      role="radiogroup"
      aria-label="Theme"
      className={cn("inline-flex gap-0.5 rounded-control bg-subtle p-0.5", className)}
    >
      {OPTIONS.map(({ value, label, Icon }) => (
        <label
          key={value}
          className={cn(
            "relative inline-flex h-9 w-9 cursor-pointer items-center justify-center rounded-md text-muted has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-accent has-[:focus-visible]:shadow-[0_0_0_5px_var(--focus-halo)]",
            pref === value && "bg-surface text-ink shadow-card",
          )}
        >
          <input
            type="radio"
            name={name}
            value={value}
            checked={pref === value}
            onChange={() => setPref(value)}
            aria-label={label}
            className="absolute inset-0 m-0 cursor-pointer opacity-0"
          />
          <Icon className="size-4" />
        </label>
      ))}
    </div>
  );
}
