"use client";
import { ChevronDown, ChevronRight, Eye, Lightbulb, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { TypeDot } from "@/components/type-dot";
import { Button } from "@/components/ui/button";
import type { Theme } from "@/lib/graph/colors";
import type { ApiError, Hit, SearchResult } from "@/lib/types";
import { cn } from "@/lib/utils";

/** Where a hit is on the canvas: shown, hidden (with why), or not in this view at all (a truncated snapshot). */
export type Placement = { kind: "shown" } | { kind: "hidden"; reasons: string[] } | { kind: "absent" };

type Props = {
  result: SearchResult;
  theme: Theme;
  place: (id: string) => Placement;
  /** Select the node; `pan` moves the camera to it. */
  onPick: (id: string) => void;
  /** Undo what hides the node, then select it and move to it. */
  onReveal: (id: string) => void;
  testId: string;
  /** With Merge entities on (PRD 005 centre-15): a hit's entity, so the list names it once. */
  entityOf?: HitEntities | null;
  /** Select one member of an entity on its own: only its links (PRD 005 centre-15). */
  onPickMember?: (id: string) => void;
};

/** A merged entity's shown members (two or more), each with its graph and title, or null for a node in none. */
export type HitEntities = (id: string) => { entity: number; members: { key: string; graph: string; title: string }[] } | null;
export type HitRow = { hit: Hit; entity: number | null; members: { key: string; graph: string; title: string }[] };

/** One row per hit, except that the hits of one merged entity make one row, at the place of its best hit. */
export function hitRows(hits: readonly Hit[], entityOf?: HitEntities | null): HitRow[] {
  const seen = new Set<number>();
  const out: HitRow[] = [];
  for (const hit of hits) {
    const e = entityOf?.(hit.id) ?? null;
    if (!e) out.push({ hit, entity: null, members: [] });
    else if (!seen.has(e.entity)) {
      seen.add(e.entity);
      out.push({ hit, entity: e.entity, members: e.members });
    }
  }
  return out;
}

export function Warnings({ warnings, testId }: { warnings: ApiError[]; testId: string }) {
  if (warnings.length === 0) return null;
  return (
    <ul className="space-y-1" data-testid={testId}>
      {warnings.map((w, i) => (
        <li key={i} className="flex gap-1.5 rounded-md border border-amber-500/40 bg-amber-50 px-2 py-1 text-xs text-amber-900 dark:bg-amber-950 dark:text-amber-100" data-code={w.code}>
          <TriangleAlert className="mt-0.5 size-3 shrink-0" aria-hidden />
          <span className="min-w-0">
            <span className="font-mono">{w.code}</span>: {w.message}
            {w.next ? <span className="block font-mono break-all opacity-80 select-all" data-testid="hint-next">{w.next}</span> : null}
          </span>
        </li>
      ))}
    </ul>
  );
}

/** One hint line: umx's words and its next command, and how many replies carried it. */
export type HintItem = ApiError & { count: number };

/**
 * umx's hints (D69 (9)): the envelope's hints, and every warning that names a next command, each once (by code and
 * next) with a count. Only what umx said: the view never makes one up.
 */
export function hintItems(warnings: readonly ApiError[], hints: readonly ApiError[] = []): HintItem[] {
  const out = new Map<string, HintItem>();
  for (const h of [...hints, ...warnings.filter((w) => w.next)]) {
    const key = `${h.code}\u0000${h.next ?? h.message}`;
    const had = out.get(key);
    if (had) had.count++;
    else out.set(key, { ...h, count: 1 });
  }
  return [...out.values()];
}

export function Hints({ items, testId, max = 5 }: { items: readonly HintItem[]; testId: string; max?: number }) {
  if (items.length === 0) return null;
  return (
    <ul className="space-y-1" data-testid={testId}>
      {items.slice(0, max).map((h, i) => (
        <li key={i} className="flex gap-1.5 rounded-md border bg-card px-2 py-1 text-xs" data-code={h.code}>
          <Lightbulb className="mt-0.5 size-3 shrink-0 text-primary" aria-hidden />
          <span className="min-w-0">
            {h.message}
            {h.count > 1 ? <span className="text-muted-foreground tabular-nums"> (×{h.count})</span> : null}
            {h.next ? <span className="block font-mono break-all text-muted-foreground select-all">{h.next}</span> : null}
          </span>
        </li>
      ))}
      {items.length > max ? <li className="text-[11px] text-muted-foreground">{items.length - max} more</li> : null}
    </ul>
  );
}

function score(hit: Hit): string {
  if (hit.score === null) return "";
  return hit.score >= 10 ? hit.score.toFixed(0) : hit.score.toFixed(hit.score < 0.1 ? 4 : 3);
}

export function HitList({ result, theme, place, onPick, onReveal, testId, entityOf = null, onPickMember }: Props) {
  const [open, setOpen] = useState<ReadonlySet<number>>(new Set());
  const rows = hitRows(result.hits, entityOf);
  return (
    <div className="space-y-1.5">
      <Warnings warnings={result.warnings} testId={`${testId}-warnings`} />
      <Hints items={hintItems([], result.hints)} testId={`${testId}-hints`} />
      {result.note ? <p className="text-[11px] text-muted-foreground">{result.note}</p> : null}
      {result.quiet ? (
        <p className="text-[11px] text-muted-foreground" data-testid={`${testId}-quiet`}>
          {result.quiet}
        </p>
      ) : null}
      {result.hits.length === 0 ? (
        <p className="text-xs text-muted-foreground">No matches.</p>
      ) : (
        <ul className="space-y-0.5" data-testid={testId}>
          {rows.map(({ hit, entity, members }) => {
            const where = place(hit.id);
            const expanded = entity !== null && open.has(entity);
            const graphs = [...new Set(members.map((m) => m.graph))];
            return (
              <li key={hit.id} data-id={hit.id} data-place={where.kind} data-entity={entity ?? undefined}>
                <div className="flex items-center">
                {entity !== null ? (
                  <button
                    type="button"
                    aria-label={expanded ? "Hide members" : "Show members"}
                    aria-expanded={expanded}
                    onClick={() => setOpen((s) => {
                      const next = new Set(s);
                      if (!next.delete(entity)) next.add(entity);
                      return next;
                    })}
                    className="rounded-sm p-0.5 text-muted-foreground hover:bg-muted"
                    data-testid="hit-expand"
                  >
                    {expanded ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
                  </button>
                ) : null}
                <button
                  type="button"
                  onClick={() => onPick(hit.id)}
                  className="flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1 text-left text-sm hover:bg-muted"
                >
                  <TypeDot type={hit.type} theme={theme} />
                  <span className={cn("min-w-0 flex-1 truncate", where.kind !== "shown" && "text-muted-foreground")}>{hit.title}</span>
                  {entity !== null ? (
                    <span className="max-w-[96px] truncate text-[10px] text-muted-foreground" data-testid="hit-graphs" title={graphs.join(", ")}>{graphs.join(", ")}</span>
                  ) : hit.graph ? <span className="max-w-[72px] truncate text-[10px] text-muted-foreground" data-testid="hit-graph">{hit.graph}</span> : null}
                  {hit.legs ? <span className="font-mono text-[10px] text-muted-foreground">{hit.exact ? `exact ${hit.exact}` : hit.legs}</span> : null}
                  <span className="text-[10px] text-muted-foreground tabular-nums">{score(hit)}</span>
                </button>
                </div>
                {expanded ? (
                  <ul className="pl-6" data-testid="hit-members">
                    {members.map((m) => (
                      <li key={m.key}>
                        <button type="button" onClick={() => (onPickMember ?? onPick)(m.key)} className="flex w-full items-center gap-2 rounded-md px-2 py-0.5 text-left text-xs hover:bg-muted" data-testid="hit-member" data-id={m.key}>
                          <span className="min-w-0 flex-1 truncate">{m.title}</span>
                          <span className="font-mono text-[10px] text-muted-foreground">{m.graph}</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                ) : null}
                {where.kind === "hidden" ? (
                  <div className="flex items-center gap-1.5 pr-1 pl-6 text-[11px] text-muted-foreground" data-testid="hit-hidden">
                    <span className="min-w-0 flex-1">Hidden: {where.reasons.join(", ")}</span>
                    <Button variant="ghost" size="sm" className="h-5 gap-1 px-1.5 text-[11px] [&_svg]:size-3" onClick={() => onReveal(hit.id)} data-testid="hit-show">
                      <Eye className="size-3" /> Show
                    </Button>
                  </div>
                ) : where.kind === "absent" ? (
                  <p className="pl-6 text-[11px] text-muted-foreground">Not in this view: the graph opened truncated.</p>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
      {result.foreign > 0 ? <p className="text-[11px] text-muted-foreground">{result.foreign} rows from other graphs not listed.</p> : null}
    </div>
  );
}
