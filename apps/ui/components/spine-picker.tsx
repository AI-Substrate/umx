"use client";
// The spine view's control (Jordan's brief part 2): per open graph with declared spines, None or one spine. A graph
// shows its main spine (else its first) until the person picks; None returns it to the force layout.
import { CalendarDays, ListOrdered } from "lucide-react";
import { SPREAD } from "@/lib/graph/spine-layout";
import type { SpineInfo } from "@/lib/types";
import { cn } from "@/lib/utils";

export type SpineRow = {
  alias: string;
  spines: SpineInfo[] | undefined;
  shown: string | null;
  /** Placements are being read, or this umx cannot give them (older than `node list --columns spine:<name>`). */
  state: "ready" | "loading" | "unsupported" | "failed" | "gone";
};

export type SpinePanel = {
  rows: SpineRow[];
  multi: boolean;
  onPick: (alias: string, name: string | null) => void;
  /** The Spread slider: the gap between bands and between a band's nodes, while a spine is shown. */
  spread: number;
  onSpread: (s: number) => void;
};

export function SpinePicker({ rows, multi, onPick, spread, onSpread }: SpinePanel) {
  const withSpines = rows.filter((r) => (r.spines?.length ?? 0) > 0);
  if (withSpines.length === 0) return null;
  return (
    <div className="space-y-2 border-t px-4 py-3" data-testid="spine-picker">
      <h2 className="section-head">Spine</h2>
      {withSpines.map((r) => (
        <div key={r.alias} className="space-y-1" data-graph={r.alias}>
          {multi ? <p className="truncate text-[11px] text-muted-foreground">{r.alias}</p> : null}
          <div className="flex flex-wrap gap-1" role="radiogroup" aria-label={`Spine for ${r.alias}`}>
            <Chip on={r.shown === null} onClick={() => onPick(r.alias, null)} title="The force layout, with no spine">
              None
            </Chip>
            {r.spines!.map((s) => (
              <Chip key={s.name} on={r.shown === s.name} onClick={() => onPick(r.alias, s.name)} title={`${s.title}: ${s.cover.total - s.cover.notPlaced} of ${s.cover.total} nodes placed${s.main ? " · main spine" : ""}`} testId="spine-chip">
                {s.clock === "calendar" ? <CalendarDays className="size-3" aria-hidden /> : <ListOrdered className="size-3" aria-hidden />}
                {s.name}
              </Chip>
            ))}
          </div>
          {r.shown && r.state === "loading" ? <p className="text-[11px] text-muted-foreground">Placing nodes…</p> : null}
          {r.shown && r.state === "unsupported" ? (
            <p className="text-[11px] text-muted-foreground">This umx cannot place nodes on a spine yet; update umx to lay the graph out along it.</p>
          ) : null}
          {r.shown && r.state === "gone" ? (
            <p className="text-[11px] text-muted-foreground">The graph no longer declares this spine (`umx graph spine show {r.alias}`).</p>
          ) : null}
          {r.shown && r.state === "failed" ? <p className="text-[11px] text-destructive">The spine’s placements could not be read.</p> : null}
        </div>
      ))}
      {withSpines.some((r) => r.shown !== null) ? (
        <label className="block space-y-1 pt-1 text-sm">
          <span className="flex items-center justify-between">
            <span>Spread</span>
            <span className="text-xs text-muted-foreground tabular-nums">{spread.toFixed(1)}</span>
          </span>
          <input
            type="range"
            min={SPREAD.min}
            max={SPREAD.max}
            step={SPREAD.step}
            value={spread}
            onChange={(e) => onSpread(Number(e.target.value))}
            className="w-full accent-primary"
            aria-label="Spine spread"
            title="Spread the bands and the nodes in each band apart; Fit frames the result"
            data-testid="spine-spread"
          />
        </label>
      ) : null}
      <p className="text-[11px] text-muted-foreground">Earliest on the left. Nodes the spine does not place sit on a shelf below.</p>
    </div>
  );
}

function Chip({ on, onClick, title, children, testId }: { on: boolean; onClick: () => void; title: string; children: React.ReactNode; testId?: string }) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={on}
      onClick={onClick}
      title={title}
      className={cn(
        "flex h-7 items-center gap-1.5 rounded-full border px-2.5 text-xs transition-colors duration-150",
        on ? "border-primary bg-primary text-primary-foreground" : "text-muted-foreground hover:bg-muted hover:text-foreground",
      )}
      data-testid={testId}
      data-on={on || undefined}
    >
      {children}
    </button>
  );
}
