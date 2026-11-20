import { AppShell } from "@/components/app-shell";
import { NotFoundState } from "@/components/not-found-state";

// (P5-REFINE-2, org-3) Any unknown path: "That page doesn't exist" inside the app shell, with the viewer's own rail.
export default function NotFound() {
  return (
    <AppShell>
      <NotFoundState kind="page" />
    </AppShell>
  );
}
