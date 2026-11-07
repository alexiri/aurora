import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-control text-sm font-semibold transition-colors disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50",
  {
    variants: {
      variant: {
        primary: "bg-primary text-on-primary hover:opacity-90",
        outline: "border border-line-strong bg-surface text-ink hover:bg-subtle",
        approve: "bg-accent text-on-accent hover:opacity-90",
        ghost: "text-ink-2 hover:bg-subtle hover:text-ink",
        danger: "border border-danger-ink bg-surface text-danger-ink hover:bg-danger-soft",
      },
      size: {
        sm: "min-h-9 px-3",
        default: "min-h-10 px-4",
        lg: "min-h-11 px-5 text-[15px]",
      },
    },
    defaultVariants: { variant: "primary", size: "default" },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

function Button({ className, variant, size, asChild = false, ...props }: ButtonProps) {
  const Comp = asChild ? Slot : "button";
  return <Comp className={cn(buttonVariants({ variant, size, className }))} {...props} />;
}

export { Button, buttonVariants };
