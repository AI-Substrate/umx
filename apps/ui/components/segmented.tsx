"use client";
import { cn } from "@/lib/utils";

/** A row of mutually exclusive buttons (colour by, grouping, focus depth). */
export function Segmented<T extends string | number>({
  value,
  options,
  onChange,
  label,
  testId,
  disabled,
}: {
  value: T;
  options: readonly (readonly [T, string])[];
  onChange: (v: T) => void;
  label: string;
  testId?: string;
  disabled?: boolean;
}) {
  return (
    <div role="radiogroup" aria-label={label} data-testid={testId} className="inline-flex rounded-md border bg-muted/50 p-0.5">
      {options.map(([v, text]) => (
        <button
          key={String(v)}
          type="button"
          role="radio"
          aria-checked={v === value}
          disabled={disabled}
          data-value={String(v)}
          onClick={() => onChange(v)}
          className={cn(
            "rounded-[5px] px-2 py-0.5 text-xs text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50",
            v === value && "bg-card text-foreground shadow-sm",
          )}
        >
          {text}
        </button>
      ))}
    </div>
  );
}
