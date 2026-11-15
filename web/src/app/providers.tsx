"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useParams, usePathname } from "next/navigation";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { configureTelemetry, routeTemplate, traceNavigation } from "@/lib/telemetry";

/**
 * (FIX-146, D-232 (i)) `navigate <template>` per App Router navigation, ending at the first commit of the new
 * page. It starts at the click that caused it when there was one in the last 2 s (links are plain anchors for
 * the router), else when the new pathname first renders. Inert when tracing is off.
 */
function NavigationTracer() {
  const pathname = usePathname();
  const params = useParams();
  const first = useRef(true);
  const lastClick = useRef(0);
  useEffect(() => {
    const onClick = () => {
      lastClick.current = performance.timeOrigin + performance.now();
    };
    document.addEventListener("click", onClick, true);
    return () => document.removeEventListener("click", onClick, true);
  }, []);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return; // the initial load is the browser's, not an App Router navigation
    }
    const now = performance.timeOrigin + performance.now();
    traceNavigation(routeTemplate(pathname, params), now - lastClick.current < 2000 ? lastClick.current : undefined);
  }, [pathname, params]);
  return null;
}

export function Providers({ children, tracingRatio = null }: { children: ReactNode; tracingRatio?: number | null }) {
  const [client] = useState(
    () => new QueryClient({ defaultOptions: { queries: { staleTime: 30_000, retry: 1 } } }),
  );
  // Synchronously, before any child can call the API client: the ratio is the server-rendered env value.
  useState(() => configureTelemetry(tracingRatio));
  return (
    <QueryClientProvider client={client}>
      {tracingRatio !== null ? <NavigationTracer /> : null}
      {children}
    </QueryClientProvider>
  );
}
