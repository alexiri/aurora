import type { ComponentProps, ReactNode } from "react";

import { inputClass } from "@/components/ui/field";
import { cn } from "@/lib/utils";

export interface SelectOption {
  value: string;
  label: ReactNode;
}

/**
 * The dashboard's one drop-down: the input look (`inputClass`, merged with `className`), a value-based `onChange`, and the
 * choices as `options` (an optional first empty-valued `placeholder` row, then one row per option). `children` takes
 * grouped or irregular rows instead. Give it an `id` and a `<Field>` label, or an `aria-label`.
 */
export function Select({
  options,
  placeholder,
  onChange,
  className,
  children,
  ...props
}: Omit<ComponentProps<"select">, "onChange"> & {
  options?: readonly SelectOption[];
  placeholder?: string;
  onChange: (value: string) => void;
}) {
  return (
    <select className={cn(inputClass, className)} onChange={(e) => onChange(e.target.value)} {...props}>
      {placeholder === undefined ? null : <option value="">{placeholder}</option>}
      {options?.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
      {children}
    </select>
  );
}
