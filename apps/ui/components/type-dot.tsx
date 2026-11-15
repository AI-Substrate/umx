import { type Theme, typeColor } from "@/lib/graph/colors";
import { shapeOf, svgPath } from "@/lib/graph/shapes";
import { cn } from "@/lib/utils";

/** A type's mark: its node shape (lib/graph/shapes), filled with its type colour. */
export function TypeDot({ type, theme, className }: { type: string; theme: Theme; className?: string }) {
  const shape = shapeOf(type);
  return (
    <svg aria-hidden viewBox="0 0 10 10" className={cn("inline-block size-2.5 shrink-0", className)} data-shape={shape}>
      <path d={svgPath(shape, 10)} fill={typeColor(type, theme)} />
    </svg>
  );
}
