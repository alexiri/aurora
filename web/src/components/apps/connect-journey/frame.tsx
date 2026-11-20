"use client";

import { createContext, useContext, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";

/**
 * (P5-POLISH-5, D-388) The one frame of both connect journeys: a sheet that is full screen below 640 px and a 600 px
 * panel above, so the header (title and steps) and the footer never move between steps. The footer is a slot the
 * steps fill (`StepFooter`): Back on the left, the step's one action on the right.
 */
const FooterContext = createContext<HTMLElement | null>(null);

/** Renders its children into the frame's footer. */
export function StepFooter({ children }: { children: ReactNode }) {
  const el = useContext(FooterContext);
  return el ? createPortal(children, el) : null;
}

/** Back on the left (absent on the first step), the step's one action on the right. */
export function BackNext({
  onBack,
  onNext,
  nextLabel = "Next",
  nextDisabled,
  busy,
  busyLabel = "Saving…",
  form,
}: {
  onBack?: () => void;
  onNext?: () => void;
  nextLabel?: string;
  nextDisabled?: boolean;
  busy?: boolean;
  busyLabel?: string;
  /** The id of the form the action submits (the Sign in step). */
  form?: string;
}) {
  return (
    <StepFooter>
      {onBack ? (
        <Button type="button" variant="ghost" onClick={onBack}>
          Back
        </Button>
      ) : null}
      <Button
        type={form ? "submit" : "button"}
        form={form}
        size="lg"
        className="ml-auto"
        disabled={nextDisabled || busy}
        onClick={form ? undefined : onNext}
      >
        {busy ? (
          <>
            <span aria-hidden="true" className="mr-2 inline-block size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent motion-reduce:animate-none" />
            {busyLabel}
          </>
        ) : (
          nextLabel
        )}
      </Button>
    </StepFooter>
  );
}

export function JourneyFrame({
  title,
  testId,
  stepper,
  back,
  footer,
  onClose,
  children,
}: {
  title: string;
  testId: string;
  /** The steps, in the header. */
  stepper: ReactNode;
  /** The sheet's back arrow (the same as Back); absent on the first step. */
  back?: ReactNode;
  /** `false` on the first step, which has no footer. */
  footer: boolean;
  onClose: () => void;
  children: ReactNode;
}) {
  const [el, setEl] = useState<HTMLElement | null>(null);
  return (
    <Dialog open onOpenChange={(o) => (o ? null : onClose())}>
      <DialogContent
        size="lg"
        variant="sheet"
        fixedHeight="600px"
        title={title}
        data-testid={testId}
        // The title is one line on a phone: it is cut with an ellipsis, not wrapped.
        className={cn("[&_h2]:truncate")}
        back={back}
        header={stepper}
        footer={footer ? <div ref={setEl} className="flex flex-wrap items-center gap-2" /> : null}
      >
        <FooterContext.Provider value={footer ? el : null}>{children}</FooterContext.Provider>
      </DialogContent>
    </Dialog>
  );
}
