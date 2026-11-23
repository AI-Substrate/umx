"use client";
// The graph picker (ws009 M1a, plan 133; D69 (7)). Only the view lives here: which graphs are open, the include-secured
// toggle and opening come in through GraphPickerProps, whose state is components/use-graph-set and whose rules are
// lib/graph-set. The sidebar shows what is open; the whole index is chosen in a dialog (G), as a draft that opens only
// when the person says so, so checking a dozen graphs reads them once.
import { Check, Layers, Lock, Search, X } from "lucide-react";
import { type KeyboardEvent, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { type Theme } from "@/lib/graph/colors";
import { graphColor } from "@/lib/graph/graph-colors";
import { draftToggled, groupOf, type LeftOut, matchGraphs, type PickGroup, scopeGraphs, selectMatching } from "@/lib/graph-set";
import { MAX_OPEN } from "@/lib/graph/keys";
import type { GraphSummary, ScopeSummary } from "@/lib/types";
import { cn, number } from "@/lib/utils";

/** What any graph picker gets: the graph index, the saved scopes, the open set, and the two ways to change it. */
export type GraphPickerProps = {
  graphs: GraphSummary[] | null;
  scopes: ScopeSummary[] | null;
  /** The graphs open on the canvas, in the order they were opened (their colour rank). */
  open: readonly string[];
  /** Opens exactly these graphs (names them); `leftOut` when a scope or "all" chose them. None clears the canvas. */
  onOpen: (aliases: string[], leftOut?: LeftOut) => void;
  /** D69 (7): while on, "Open all" and scopes open secured graphs too. Off on every load, set only by the person. */
  includeSecured: boolean;
  onIncludeSecured: (on: boolean) => void;
  /** Hold (plan 152): each open graph's held changes not yet applied (null: many), only those with some, and
   * applying one graph's. */
  held?: Held;
  /** Graphs an agent opened whose snapshot is still being read (ws009 M1b1). */
  loading?: readonly string[];
  /** The dialog, held by the app so a key (G) or the empty canvas can open it. */
  choosing: boolean;
  onChoosing: (on: boolean) => void;
  /** For each open graph's colour dot. */
  theme?: Theme;
};

export type Held = { counts: ReadonlyMap<string, number | null>; onApply: (alias: string) => void };

export function GraphPicker(p: GraphPickerProps) {
  const total = p.graphs?.length ?? 0;
  const trigger = useRef<HTMLButtonElement>(null);
  const { onChoosing } = p;
  // The dialog closes itself (dialog.close()); once it has, focus goes back to the button that opens it.
  const closed = useCallback(() => {
    onChoosing(false);
    trigger.current?.focus();
  }, [onChoosing]);
  return (
    <>
      <div className="flex items-center justify-between gap-2">
        <h2 className="section-head">Graphs</h2>
        {p.graphs ? (
          <span className="text-[11px] text-muted-foreground tabular-nums" data-testid="graph-count">
            {p.open.length} open · {number.format(total)} known
          </span>
        ) : null}
      </div>
      <Button
        ref={trigger}
        variant="outline"
        size="sm"
        className="w-full justify-between"
        onClick={() => p.onChoosing(true)}
        disabled={p.graphs === null}
        aria-haspopup="dialog"
        aria-keyshortcuts="g"
        data-testid="choose-graphs"
      >
        <span className="flex items-center gap-2">
          <Layers aria-hidden /> {p.graphs === null ? "Loading graphs…" : "Choose graphs"}
        </span>
        <Kbd>G</Kbd>
      </Button>
      {p.graphs?.length === 0 ? <p className="text-sm text-muted-foreground">No graphs are loaded or remembered. Run `umx graph load &lt;folder&gt;`.</p> : null}
      <OpenList open={p.open} onOpen={p.onOpen} held={p.held} loading={p.loading} theme={p.theme} />
      {p.choosing && p.graphs ? <GraphDialog {...p} graphs={p.graphs} onClosed={closed} /> : null}
    </>
  );
}

function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="rounded border bg-muted px-1.5 font-mono text-[10px] leading-4 text-muted-foreground">{children}</kbd>;
}

/** The open graphs in their colours, each with its held changes and a way off the canvas. */
function OpenList({ open, onOpen, held, loading = [], theme }: Pick<GraphPickerProps, "open" | "onOpen" | "held" | "loading" | "theme">) {
  if (open.length === 0) return null;
  return (
    <ul className="space-y-0.5" data-testid="open-graphs" aria-label="Open graphs">
      {open.map((a, rank) => {
        const owed = held?.counts.has(a) ? held.counts.get(a)! : undefined;
        return (
          <li key={a} className="group flex h-7 items-center gap-2 rounded-md pr-0.5 pl-2 text-sm hover:bg-muted" data-alias={a}>
            <span aria-hidden className="inline-block size-2 shrink-0 rounded-full" style={{ background: theme ? graphColor(rank, theme) : "currentColor" }} />
            <span className="min-w-0 flex-1 truncate">{a}</span>
            {loading.includes(a) ? (
              <span className="text-[10px] text-muted-foreground uppercase" data-testid="graph-loading">
                loading
              </span>
            ) : null}
            {owed !== undefined ? (
              <span className="flex shrink-0 items-center gap-1" data-testid="graph-held" data-count={owed ?? "many"}>
                <span className="text-[11px] text-amber-700 tabular-nums dark:text-amber-400 fui:font-mono fui:text-primary fui:uppercase" title="Changes held, not applied yet">
                  · {owed === null ? "many" : number.format(owed)}
                </span>
                <button
                  type="button"
                  onClick={() => held!.onApply(a)}
                  className="rounded px-1.5 py-0.5 text-[10px] text-muted-foreground hover:bg-background hover:text-foreground fui:font-mono fui:uppercase"
                  title={`Apply ${a}'s held changes`}
                  data-testid="graph-apply"
                >
                  Apply
                </button>
              </span>
            ) : null}
            <button
              type="button"
              onClick={() => onOpen(open.filter((x) => x !== a))}
              className="grid size-6 shrink-0 place-items-center rounded text-muted-foreground opacity-0 group-focus-within:opacity-100 group-hover:opacity-100 hover:bg-background hover:text-foreground focus-visible:opacity-100"
              aria-label={`Take ${a} off the canvas`}
              title="Take this graph off the canvas"
              data-testid="graph-close"
            >
              <X className="size-3.5" aria-hidden />
            </button>
          </li>
        );
      })}
    </ul>
  );
}

const GROUPS: [PickGroup, string, string][] = [
  ["loaded", "Loaded", "Open now"],
  ["idle", "Not loaded", "Opening one loads it"],
  ["blocked", "Can’t open", "Each says why and what fixes it"],
];

/** The whole index in a dialog: search, check and uncheck into a draft, then open it (or clear the canvas). */
function GraphDialog(p: GraphPickerProps & { graphs: GraphSummary[]; onClosed: () => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState<string[]>(() => [...p.open]);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    const d = ref.current;
    if (d && !d.open) d.showModal();
    searchRef.current?.focus();
  }, []);

  const matching = useMemo(() => matchGraphs(p.graphs, query), [p.graphs, query]);
  const groups = GROUPS.map(([key, title, sub]) => ({ key, title, sub, rows: matching.filter((g) => groupOf(g) === key) })).filter((g) => g.rows.length > 0);
  const openable = matching.filter((g) => g.openable);
  const changed = draft.join(",") !== p.open.join(",");
  const full = draft.length >= MAX_OPEN;

  const close = () => ref.current?.close();
  const apply = () => {
    if (changed) p.onOpen(draft);
    close();
  };
  const toggle = (alias: string) => {
    setNote(null);
    setDraft((d) => draftToggled(d, alias));
  };
  const rows = () => [...(listRef.current?.querySelectorAll<HTMLButtonElement>("[data-row]") ?? [])];
  const move = (from: HTMLElement | null, by: number) => {
    const all = rows();
    if (all.length === 0) return;
    const at = from ? all.indexOf(from as HTMLButtonElement) : -1;
    if (at === 0 && by < 0) return searchRef.current?.focus();
    all[Math.max(0, Math.min(all.length - 1, at + by))]?.focus();
  };
  const onKey = (e: KeyboardEvent<HTMLDialogElement>) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      apply();
    } else if (e.key === "Escape") {
      // Esc empties the search first, then closes without opening anything; it never reaches the canvas.
      e.preventDefault();
      e.stopPropagation();
      if (query !== "" && e.target === searchRef.current) setQuery("");
      else close();
    } else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && (e.target === searchRef.current || (e.target as HTMLElement).dataset.row !== undefined)) {
      e.preventDefault();
      move(e.target === searchRef.current ? null : (e.target as HTMLElement), e.key === "ArrowDown" ? 1 : -1);
    }
  };
  const selectAll = () => {
    const r = selectMatching(draft, matching, p.includeSecured);
    setDraft(r.aliases);
    const parts = [
      r.leftOut.secured > 0 ? `${r.leftOut.secured} secured left out` : null,
      r.leftOut.unavailable > 0 ? `${r.leftOut.unavailable} past the ${MAX_OPEN}-graph limit left out` : null,
    ].filter(Boolean);
    setNote(parts.length > 0 ? parts.join(", ") : null);
  };

  return (
    <dialog
      ref={ref}
      onKeyDown={onKey}
      onCancel={(e) => e.preventDefault()}
      onClose={p.onClosed}
      onClick={(e) => e.target === ref.current && close()}
      aria-labelledby="graph-dialog-title"
      className="m-auto flex h-[min(46rem,calc(100dvh-3rem))] w-[min(60rem,calc(100vw-2rem))] flex-col overflow-hidden rounded-xl border bg-card p-0 text-foreground shadow-md backdrop:bg-background/70 backdrop:backdrop-blur-[2px] motion-safe:animate-[picker-in_160ms_cubic-bezier(0.22,1,0.36,1)]"
      data-testid="graph-dialog"
    >
      <header className="flex items-start justify-between gap-4 px-5 pt-4 pb-3">
        <div>
          <h2 id="graph-dialog-title" className="text-base font-semibold">
            Choose graphs
          </h2>
          <p className="text-xs text-muted-foreground">
            {number.format(p.graphs.length)} known · up to {MAX_OPEN} open together on one canvas
          </p>
        </div>
        <Button variant="ghost" size="icon-sm" onClick={close} aria-label="Close without changing">
          <X aria-hidden />
        </Button>
      </header>

      <div className="space-y-2.5 border-b px-5 pb-3">
        <div className="relative">
          <Search className="pointer-events-none absolute top-2.5 left-2.5 size-4 text-muted-foreground" aria-hidden />
          <Input
            ref={searchRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              // Enter with one graph left checks or unchecks it.
              if (e.key === "Enter" && !e.metaKey && !e.ctrlKey && openable.length === 1) {
                e.preventDefault();
                toggle(openable[0].alias);
              }
            }}
            placeholder="Filter by name"
            className="pr-24 pl-8"
            maxLength={100}
            aria-label="Filter graphs by name"
            aria-controls="graph-dialog-list"
            data-testid="graph-filter"
          />
          <span className="pointer-events-none absolute top-2.5 right-3 text-xs text-muted-foreground tabular-nums" aria-live="polite">
            {query ? `${matching.length} of ${p.graphs.length}` : ""}
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
          <Button variant="secondary" size="sm" onClick={selectAll} disabled={!matching.some((g) => g.state === "loaded" && g.openable) || full} data-testid="select-matching">
            <Check aria-hidden /> {query ? "Select loaded matches" : "Select all loaded"}
          </Button>
          <Button variant="ghost" size="sm" onClick={() => (setDraft([]), setNote(null))} disabled={draft.length === 0} data-testid="select-none">
            Select none
          </Button>
          {note ? <span className="text-xs text-muted-foreground">{note}</span> : null}
          <Scopes scopes={p.scopes ?? []} includeSecured={p.includeSecured} onPick={(aliases, left) => (setDraft(aliases), setNote(leftNote(left)))} />
        </div>
      </div>

      <div ref={listRef} id="graph-dialog-list" className="min-h-0 flex-1 overflow-y-auto px-5 py-3" role="group" aria-label="Graphs">
        {matching.length === 0 ? (
          <div className="grid place-items-center gap-2 py-12 text-center text-sm text-muted-foreground">
            <p>No graph’s name holds “{query}”.</p>
            <Button variant="outline" size="sm" onClick={() => (setQuery(""), searchRef.current?.focus())}>
              Clear the filter
            </Button>
          </div>
        ) : (
          groups.map((g) => (
            <section key={g.key} className="mb-3 last:mb-0" data-group={g.key}>
              <h3 className="mb-1 flex items-baseline gap-2 px-2">
                <span className="text-xs font-medium">{g.title}</span>
                <span className="text-[11px] text-muted-foreground tabular-nums">{g.rows.length}</span>
                <span className="text-[11px] text-muted-foreground">· {g.sub}</span>
              </h3>
              <GraphList graphs={g.rows} open={draft} onToggle={toggle} onPick={(a) => (setDraft([a]), setNote(null))} query={query} canvas={p.open} theme={p.theme} />
            </section>
          ))
        )}
      </div>

      <footer className="flex flex-wrap items-center justify-between gap-3 border-t bg-muted/40 px-5 py-3">
        <label
          className={cn(
            "flex items-center gap-2 rounded-md px-2 py-1 text-xs",
            p.includeSecured ? "bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-100" : "text-muted-foreground",
          )}
          title="Select all and scopes include secured graphs while this is on. Off whenever the view opens; agents cannot turn it on."
          data-testid="include-secured-row"
          data-on={p.includeSecured || undefined}
        >
          <Switch checked={p.includeSecured} onCheckedChange={p.onIncludeSecured} aria-label="Include secured graphs" data-testid="include-secured" />
          <Lock className="size-3" aria-hidden />
          {p.includeSecured ? "Including secured graphs" : "Include secured graphs"}
        </label>
        <div className="flex items-center gap-3">
          <span className={cn("text-xs tabular-nums", full ? "text-amber-700 dark:text-amber-400" : "text-muted-foreground")} data-testid="draft-count">
            {draft.length} of {MAX_OPEN} selected
          </span>
          <span className="hidden text-[11px] text-muted-foreground lg:inline">
            <Kbd>↑↓</Kbd> move <Kbd>Space</Kbd> check <Kbd>⌘↵</Kbd> open
          </span>
          <Button variant="ghost" size="sm" onClick={close}>
            Cancel
          </Button>
          <Button size="sm" onClick={apply} data-testid="apply-graphs">
            {!changed ? "Done" : draft.length === 0 ? "Clear the canvas" : `Open ${draft.length} graph${draft.length === 1 ? "" : "s"}`}
          </Button>
        </div>
      </footer>
    </dialog>
  );
}

function leftNote(left: LeftOut): string | null {
  const n = left.secured + left.unavailable;
  return n > 0 ? `${n} left out (secured or unavailable)` : null;
}

/** Saved scopes (`umx scope list`): each sets the draft to its members the picker would open; a secured one only
 * while "include secured graphs" is on, else counted, never named. */
export function Scopes({ scopes, onPick, includeSecured }: { scopes: readonly ScopeSummary[]; onPick: (aliases: string[], left: LeftOut) => void; includeSecured: boolean }) {
  if (scopes.length === 0) return null;
  return (
    <div className="flex flex-wrap items-center gap-1.5 sm:ml-auto" data-testid="scope-list">
      <span className="text-[11px] text-muted-foreground">Scopes</span>
      {scopes.map((s) => {
        const opens = scopeGraphs(s, includeSecured);
        const skipped = opens.leftOut.secured + opens.leftOut.unavailable;
        return (
          <button
            key={s.name}
            type="button"
            disabled={opens.aliases.length === 0}
            onClick={() => onPick(opens.aliases, opens.leftOut)}
            className="flex h-7 items-center gap-1.5 rounded-full border px-2.5 text-xs hover:bg-muted disabled:opacity-50"
            title={`Select this scope's ${opens.aliases.length} graph${opens.aliases.length === 1 ? "" : "s"}`}
            data-scope={s.name}
          >
            {s.name}
            <span className="text-muted-foreground tabular-nums">
              {opens.aliases.length} graph{opens.aliases.length === 1 ? "" : "s"}
              {skipped > 0 ? ` (+${skipped} left out)` : ""}
            </span>
          </button>
        );
      })}
    </div>
  );
}

const STATE_LABEL: Record<string, string> = { idle: "idle", unloaded: "unloaded", not_reloaded: "not reloaded" };

/** Counts as umx gave them; for a graph that is not loaded they are from its last load. */
function Counts({ g }: { g: GraphSummary }) {
  if (g.nodes === null || g.edges === null) return null;
  const last = g.state !== "loaded";
  return (
    <span className="shrink-0 text-[11px] text-muted-foreground tabular-nums" title={last ? "Nodes · edges, from its last load" : "Nodes · edges"}>
      {number.format(g.nodes)} · {number.format(g.edges)}
      {last ? " (last load)" : ""}
    </span>
  );
}

/** `alias` with the filter's words marked. */
function Name({ alias, query }: { alias: string; query: string }) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return <span className="min-w-0 flex-1 truncate font-medium">{alias}</span>;
  const lower = alias.toLowerCase();
  const hit = new Array<boolean>(alias.length).fill(false);
  for (const w of words) for (let i = lower.indexOf(w); i >= 0; i = lower.indexOf(w, i + 1)) hit.fill(true, i, i + w.length);
  const parts: ReactNode[] = [];
  for (let i = 0; i < alias.length; ) {
    let j = i;
    while (j < alias.length && hit[j] === hit[i]) j++;
    parts.push(hit[i] ? <mark key={i} className="rounded-sm bg-primary/15 text-foreground">{alias.slice(i, j)}</mark> : alias.slice(i, j));
    i = j;
  }
  return <span className="min-w-0 flex-1 truncate font-medium">{parts}</span>;
}

/**
 * This machine's graphs (`umx graph list --all`) as checkable rows. Loaded and idle graphs open: a named read loads an
 * idle one on demand. A click checks or unchecks a graph in `open` (the draft); "only" makes it the only one.
 * Unloaded, not reloaded and moved ones are dimmed and never opened; they say why and what fixes it. The view never
 * loads a graph by itself.
 */
export function GraphList({
  graphs,
  open,
  onPick,
  onToggle,
  query = "",
  canvas = [],
  theme,
}: {
  graphs: GraphSummary[];
  open: readonly string[];
  onPick: (alias: string) => void;
  onToggle?: (alias: string) => void;
  query?: string;
  /** The graphs on the canvas now, for their colour dots. */
  canvas?: readonly string[];
  theme?: Theme;
}) {
  const full = open.length >= MAX_OPEN;
  return (
    <ul className="grid grid-cols-1 gap-x-4 gap-y-0.5 md:grid-cols-2" data-testid="graph-list">
      {graphs.map((g, i) => {
        const state = g.folder !== "present" ? g.folder.replace("_", " ") : STATE_LABEL[g.state];
        // Only a row that opens can be the open one: another folder's row with the same alias is not.
        const isOpen = g.openable && open.includes(g.alias);
        const rank = g.openable ? canvas.indexOf(g.alias) : -1;
        const head = (
          <>
            <Name alias={g.alias} query={query} />
            {g.secured ? <Lock className="size-3 shrink-0 text-muted-foreground" aria-label="secured" /> : null}
            {state ? <span className="shrink-0 text-[10px] text-muted-foreground uppercase">{state}</span> : null}
            <Counts g={g} />
          </>
        );
        const blocked = !isOpen && full;
        return (
          <li key={`${g.id}:${g.alias}:${i}`} data-state={g.state} data-folder={g.folder} data-openable={g.openable} data-open={isOpen || undefined}>
            {g.openable ? (
              <div className={cn("group flex items-center gap-1 rounded-md", isOpen ? "bg-accent text-accent-foreground" : "hover:bg-muted")}>
                <button
                  type="button"
                  role="checkbox"
                  aria-checked={isOpen}
                  aria-disabled={blocked || undefined}
                  data-row=""
                  onClick={() => (onToggle && !blocked ? onToggle(g.alias) : !onToggle ? onPick(g.alias) : undefined)}
                  className={cn(
                    "flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-left text-sm outline-none focus-visible:ring-[3px] focus-visible:ring-ring/40",
                    blocked && "cursor-not-allowed opacity-60",
                  )}
                  title={blocked ? `Up to ${MAX_OPEN} graphs open together` : g.state === "loaded" ? undefined : "Opening it loads it"}
                  data-testid="graph-toggle"
                >
                  <span
                    aria-hidden
                    className={cn(
                      "grid size-4 shrink-0 place-items-center rounded-[4px] border transition-colors duration-150",
                      isOpen ? "border-primary bg-primary text-primary-foreground" : "border-input bg-card",
                    )}
                  >
                    {isOpen ? <Check className="size-3" strokeWidth={3} /> : null}
                  </span>
                  {theme && rank >= 0 ? (
                    <span aria-label="on the canvas" className="inline-block size-2 shrink-0 rounded-full" style={{ background: graphColor(rank, theme) }} />
                  ) : null}
                  {head}
                </button>
                {onToggle ? (
                  <button
                    type="button"
                    onClick={() => onPick(g.alias)}
                    className="mr-1 shrink-0 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground opacity-0 group-focus-within:opacity-100 group-hover:opacity-100 hover:bg-background hover:text-foreground focus-visible:opacity-100"
                    title="Select only this graph"
                    tabIndex={-1}
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
