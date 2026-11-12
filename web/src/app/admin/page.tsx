import { redirect } from "next/navigation";

// FIX-048: the org selector's "All organizations" entry links /admin (docs/14:164), which had no
// route and 404'd. The admin section's first page is docs/14:758's Overview — unimplemented (its
// API, GET /v1/admin/overview, is C-459-partial), so land on the sidebar's first entry, Setup.
export default function AdminIndexPage() {
  redirect("/admin/setup");
}
