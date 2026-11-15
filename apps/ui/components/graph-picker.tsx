"use client";
// The graph picker (ws009 M1a, plan 133; D69 (7)). Only the view lives here: which graphs are open, the include-secured
// toggle and opening come in through GraphPickerProps, whose state is components/use-graph-set and whose rules are
// lib/graph-set. A replacement picker (a panel or modal) implements the same props and nothing else changes.
import { Layers, Lock } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { allGraphs, type LeftOut, scopeGraphs, toggled } from "@/lib/graph-set";
import { MAX_OPEN } from "@/lib/graph/keys";
import type { GraphSummary, ScopeSummary } from "@/lib/types";
import { cn, number } from "@/lib/utils";

/** What any graph picker gets: the graph index, the saved scopes, the open set, and the two ways to change it. */
export type GraphPickerProps = {
  graphs: GraphSummary[] | null;
  scopes: ScopeSummary[] | null;
  /** The graphs open on the canvas, in the order they were opened (their colour rank). */
  open: readonly string[];
  /** Opens exactly these graphs (names them); `leftOut` when a scope or "all" chose them. */
  onOpen: (aliases: string[], leftOut?: LeftOut) => void;
  /** D69 (7): while on, "Open all" and scopes open secured graphs too. Off on every load, set only by the person. */
  includeSecured: boolean;
  onIncludeSecured: (on: boolean) => void;
  /** Hold (plan 152): each open graph's held changes not yet applied (null: many), only those with some, and
   * applying one graph's. */
  held?: Held;
  /** Graphs an agent opened whose snapshot is still being read (ws009 M1b1). */
  loading?: readonly string[];
};

export type Held = { counts: ReadonlyMap<string, number | null>; onApply: (alias: string) => void };

export function GraphPicker(p: GraphPickerProps) {
  return (
    <>
      <div className="flex items-center justify-between gap-2">
        <h2 className="section-head">Graphs</h2>
        <OpenAll graphs={p.graphs ?? []} onOpen={p.onOpen} includeSecured={p.includeSecured} />
      </div>
      {p.graphs === null ? <p className="text-sm text-muted-foreground">Loading…</p> : null}
      {p.graphs?.length === 0 ? <p className="text-sm text-muted-foreground">No graphs are loaded or remembered. Run `umx graph load &lt;folder&gt;`.</p> : null}
      <GraphList graphs={p.graphs ?? []} open={p.open} onPick={(a) => p.onOpen([a])} onToggle={(a) => p.onOpen(toggled(p.open, a))} held={p.held} loading={p.loading} />
      <ScopeList scopes={p.scopes ?? []} onOpen={p.onOpen} includeSecured={p.includeSecured} />
      <label
        className={cn(
          "flex items-center justify-between gap-2 rounded-md px-1.5 py-1 text-xs",
          p.includeSecured ? "bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-100" : "text-muted-foreground",
        )}
        title="Open all and scopes include secured graphs while this is on. Off whenever the view opens; agents cannot turn it on."
        data-testid="include-secured-row"
        data-on={p.includeSecured || undefined}
      >
        <span className="flex items-center gap-1.5">
          <Lock className="size-3" aria-hidden />
          {p.includeSecured ? "Including secured graphs" : "Include secured graphs"}
        </span>
        <Switch checked={p.includeSecured} onCheckedChange={p.onIncludeSecured} aria-label="Include secured graphs" data-testid="include-secured" />
      </label>
    </>
  );
}

const STATE_LABEL: Record<string, string> = { idle: "idle", unloaded: "unloaded", not_reloaded: "not reloaded" };

/** Counts as umx gave them; for a graph that is not loaded they are from its last load. */
function Counts({ g }: { g: GraphSummary }) {
  if (g.nodes === null || g.edges === null) return null;
  const last = g.state !== "loaded";
  return (
    <span className="text-[11px] text-muted-foreground tabular-nums" title={last ? "From its last load" : undefined}>
      {number.format(g.nodes)} · {number.format(g.edges)}
      {last ? " (last load)" : ""}
    </span>
  );
}

function OpenAll({ graphs, onOpen, includeSecured }: { graphs: readonly GraphSummary[]; onOpen: GraphPickerProps["onOpen"]; includeSecured: boolean }) {
  const all = allGraphs(graphs, includeSecured);
  if (all.aliases.length < 2) return null;
  return (
    <button
      type="button"
      onClick={() => onOpen(all.aliases, all.leftOut)}
      className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
      title={includeSecured ? "Open every loaded graph on one canvas, secured ones included" : "Open every loaded graph that is not secured on one canvas"}
      data-testid="open-all"
    >
      <Layers className="size-3" /> Open all
    </button>
  );
}

/** Saved scopes (`umx scope list`): each opens its members the picker would open; a secured one only while "include
 * secured graphs" is on, else counted, never named. */
export function ScopeList({ scopes, onOpen, includeSecured }: { scopes: readonly ScopeSummary[]; onOpen: GraphPickerProps["onOpen"]; includeSecured: boolean }) {
  if (scopes.length === 0) return null;
  return (
    <div className="space-y-1 pt-1" data-testid="scope-list">
      <h3 className="text-[11px] text-muted-foreground">Scopes</h3>
      <ul className="space-y-0.5">
        {scopes.map((s) => {
          const opens = scopeGraphs(s, includeSecured);
          const skipped = opens.leftOut.secured + opens.leftOut.unavailable;
          return (
            <li key={s.name}>
              <button
                type="button"
                disabled={opens.aliases.length === 0}
                onClick={() => onOpen(opens.aliases, opens.leftOut)}
                className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm hover:bg-muted disabled:opacity-50"
                data-scope={s.name}
              >
                <Layers className="size-3 shrink-0 text-muted-foreground" aria-hidden />
                <span className="min-w-0 flex-1 truncate">{s.name}</span>
                <span className="text-[11px] text-muted-foreground tabular-nums">
                  {opens.aliases.length} graph{opens.aliases.length === 1 ? "" : "s"}
                  {skipped > 0 ? ` (+${skipped} left out)` : ""}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/**
 * This machine's graphs (`umx graph list --all`). Loaded and idle graphs open: a named read loads an idle one on
 * demand. A click names a graph: it adds it to the canvas, or takes it off (down to one), as the box beside it does
 * (plan 133, D69 (7)); "only" opens that graph alone.
 * Unloaded, not reloaded and moved ones are dimmed and never opened; they say why and what fixes it. The view never
 * loads a graph by itself.
 */
export function GraphList({
  graphs,
  open,
  onPick,
  onToggle,
  held,
  loading = [],
}: {
  graphs: GraphSummary[];
  open: readonly string[];
  onPick: (alias: string) => void;
  onToggle?: (alias: string) => void;
  held?: Held;
  loading?: readonly string[];
}) {
  return (
    <ul className="max-h-56 space-y-0.5 overflow-y-auto" data-testid="graph-list">
      {graphs.map((g, i) => {
        const state = g.folder !== "present" ? g.folder.replace("_", " ") : STATE_LABEL[g.state];
        // Only a row that opens can be the open one: another folder's row with the same alias is not.
        const isOpen = g.openable && open.includes(g.alias);
        const owed = isOpen && held?.counts.has(g.alias) ? held.counts.get(g.alias)! : undefined;
        const head = (
          <>
            <span className="min-w-0 flex-1 truncate font-medium">{g.alias}</span>
            {g.secured ? <Lock className="size-3 text-muted-foreground" aria-label="secured" /> : null}
            {g.openable && loading.includes(g.alias) ? (
              <span className="text-[10px] text-muted-foreground uppercase" data-testid="graph-loading">
                loading
              </span>
            ) : state ? (
              <span className="text-[10px] text-muted-foreground uppercase">{state}</span>
            ) : null}
            <Counts g={g} />
          </>
        );
        return (
          <li key={`${g.id}:${g.alias}:${i}`} data-state={g.state} data-folder={g.folder} data-openable={g.openable} data-open={isOpen || undefined}>
            {g.openable ? (
              <div className={cn("flex items-center gap-1 rounded-md pl-1.5", isOpen && "bg-accent text-accent-foreground")}>
                {onToggle ? (
                  <input
                    type="checkbox"
                    checked={isOpen}
                    disabled={(isOpen && open.length === 1) || (!isOpen && open.length >= MAX_OPEN)}
                    onChange={() => onToggle(g.alias)}
                    aria-label={isOpen ? `Take ${g.alias} off the canvas` : `Add ${g.alias} to the canvas`}
                    className="size-3.5 shrink-0 accent-primary disabled:opacity-40"
                    data-testid="graph-toggle"
                  />
                ) : null}
                <button
                  type="button"
                  onClick={() => (onToggle ? onToggle(g.alias) : onPick(g.alias))}
                  className={cn("flex min-w-0 flex-1 items-center gap-2 rounded-md px-1.5 py-1.5 text-left text-sm", !isOpen && "hover:bg-muted")}
                  title={onToggle ? (isOpen ? "Take this graph off the canvas" : "Add this graph to the canvas") : undefined}
                >
                  {head}
                </button>
                {owed !== undefined ? (
                  <span className="flex shrink-0 items-center gap-1" data-testid="graph-held" data-count={owed ?? "many"}>
                    <span className="text-[11px] text-amber-700 tabular-nums dark:text-amber-400 fui:font-mono fui:text-primary fui:uppercase" title="Changes held, not applied yet">
                      · {owed === null ? "many" : number.format(owed)}
                    </span>
                    <button
                      type="button"
                      onClick={() => held!.onApply(g.alias)}
                      className="rounded px-1.5 py-0.5 text-[10px] text-muted-foreground hover:bg-muted hover:text-foreground fui:font-mono fui:uppercase"
                      title={`Apply ${g.alias}'s held changes`}
                      data-testid="graph-apply"
                    >
                      Apply
                    </button>
                  </span>
                ) : null}
                {onToggle && !(isOpen && open.length === 1) ? (
                  <button
                    type="button"
                    onClick={() => onPick(g.alias)}
                    className="shrink-0 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground hover:bg-muted hover:text-foreground"
                    title="Open this graph alone"
                    data-testid="graph-only"
                  >
                    only
                  </button>
                ) : null}
              </div>
            ) : (
              <div className="space-y-0.5 rounded-md px-2 py-1.5 text-sm opacity-60" aria-disabled="true">
                <div className="flex items-center gap-2">{head}</div>
                <p className="text-[11px] text-muted-foreground">{g.reason}</p>
                {g.fix ? <p className="font-mono text-[11px] break-all text-muted-foreground select-all">{g.fix}</p> : null}
              </div>
            )}
          </li>
        );
      })}
    </ul>
  );
}
