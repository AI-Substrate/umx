"use client";
// The top bar's selection sharing (plan 117): "Agent can see your selection (n)" whenever the tab shares a selection,
// so sharing is never a surprise, and the pause toggle beside it (security T4b). A publish that failed, a clear
// included, is shown whatever the count or the toggle, until that graph's publish succeeds: agents may still see the
// earlier selection there (review R235-2).
import { AlertTriangle, Eye } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import type { ShareFailure } from "@/lib/selection";
import { number } from "@/lib/utils";

type Props = { count: number; sharing: boolean; failures: readonly ShareFailure[]; onSharing: (on: boolean) => void };

export function SelectionShare({ count, sharing, failures, onSharing }: Props) {
  const graphs = failures.map((f) => f.graph).join(", ");
  return (
    <div className="flex items-center gap-2 text-xs text-muted-foreground" data-testid="selection-share" data-sharing={sharing ? "on" : "paused"} data-count={count}>
      {failures.length > 0 ? (
        <span
          className="flex items-center gap-1 text-amber-700 dark:text-amber-300"
          data-testid="selection-error"
          data-code={failures[0].error.code}
          data-graphs={graphs}
          title={failures.map((f) => `${f.graph}: ${f.error.message}`).join("\n")}
        >
          <AlertTriangle className="size-3.5" aria-hidden />
          <span>
            Could not update what agents see in <bdi>{graphs}</bdi>: they may still see your previous selection. Retrying.
          </span>
        </span>
      ) : sharing && count > 0 ? (
        <span className="flex items-center gap-1 font-medium text-foreground" data-testid="selection-shared">
          <Eye className="size-3.5 text-primary" aria-hidden />
          <span>
            Agent can see your selection (<span data-testid="selection-count">{number.format(count)}</span>)
          </span>
        </span>
      ) : null}
      <label className="flex items-center gap-1.5" title="While off, agents see no selection from this tab, and what they saw is cleared">
        <Switch checked={sharing} onCheckedChange={onSharing} aria-label="Share selection with agents" data-testid="share-selection" />
        Share selection
      </label>
    </div>
  );
}
