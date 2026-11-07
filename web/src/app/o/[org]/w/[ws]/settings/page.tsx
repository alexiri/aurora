"use client";

import { useParams } from "next/navigation";

import { SettingsView } from "@/components/settings/settings-view";

export default function WorkspaceSettingsPage() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  return <SettingsView org={org} ws={ws} />;
}
