import { Button } from "@/components/ui/button";
import { PlusIcon } from "@/components/ui/icons";

/** The Skills pages' header action (skill-6): starts a new skill in the list below. */
export function AddSkillButton({ onClick }: { onClick: () => void }) {
  return (
    <Button type="button" onClick={onClick}>
      <PlusIcon className="size-4" />
      Add skill
    </Button>
  );
}
