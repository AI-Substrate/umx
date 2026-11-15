"use client";
// The shadow legend row under the Relations chips (ws009 M1c, ui-expert point 5): the dotted swatch, how many are drawn,
// the Off / Selected / All control, a refresh, and umx's notes (graphs not compared, an uncalibrated model, too many).
import { RefreshCw } from "lucide-react";
import { Segmented } from "@/components/segmented";
import type { ShadowMode } from "@/lib/graph/shadows";

export type ShadowLegend = {
  mode: ShadowMode;
  onMode: (m: ShadowMode) => void;
  /** Drawn now, and past the All cap. */
  count: number;
  more: number;
  loading: boolean;
  notes: string[];
  onRefresh: () => void;
};

const MODES = [
  ["off", "Off"],
  ["selected", "Selected"],
  ["all", "All"],
] as const;

export function ShadowLegendRow({ mode, onMode, count, more, loading, notes, onRefresh }: ShadowLegend) {
  return (
    <div className="space-y-1.5" data-testid="shadow-legend">
      <div className="flex items-center gap-2 text-xs">
        <span className="font-mono tracking-widest text-muted-foreground" aria-hidden>
          ···
        </span>
        <span className="flex-1">
          similar, not linked
          {mode !== "off" ? (
            <span className="ml-1 text-muted-foreground tabular-nums" data-testid="shadow-count" data-count={count} data-more={more}>
              {count}
              {more > 0 ? ` +${more} shadows` : ""}
            </span>
          ) : null}
        </span>
        <button type="button" className="rounded p-0.5 text-muted-foreground hover:text-foreground disabled:opacity-40" onClick={onRefresh} disabled={mode === "off" || loading} aria-label="Refresh shadows" title="Refresh shadows" data-testid="shadow-refresh">
          <RefreshCw className={loading ? "size-3 animate-spin" : "size-3"} />
        </button>
      </div>
      <Segmented<ShadowMode> label="Shadows" testId="shadow-mode" value={mode} options={MODES} onChange={onMode} />
      {mode !== "off"
        ? notes.map((n) => (
            <p key={n} className="text-[11px] text-muted-foreground" data-testid="shadow-note">
              {n}
            </p>
          ))
        : null}
    </div>
  );
}
