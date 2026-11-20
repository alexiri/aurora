"use client";

import { BackToAgent } from "@/components/account/back-link";
import { SshKeysView } from "@/components/account/ssh-keys-view";

export default function SshKeysPage() {
  return (
    <>
      <BackToAgent />
      <SshKeysView />
    </>
  );
}
