"use client";
import { Loader2, RotateCcw, Search } from "lucide-react";
import { GraphPicker, type GraphPickerProps } from "@/components/graph-picker";
import { type SpinePanel, SpinePicker } from "@/components/spine-picker";
import { HitList, type Placement } from "@/components/hit-list";
import { TypeDot } from "@/components/type-dot";
import { Segmented } from "@/components/segmented";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { type ShadowLegend, ShadowLegendRow } from "@/components/shadow-legend";
import type { SearchState } from "@/components/use-search";
import { type ChipCounts, chipList, type ColourBy, type ViewGraph } from "@/lib/graph/build";
import type { Groups } from "@/lib/graph/clusters";
import type { EdgeMode } from "@/lib/graph/edges";
import { groupColor, HULL_MIN, statusColor, statusLook, STATUS_KINDS, type StatusKind, type Theme } from "@/lib/graph/colors";
import { anyOff, type FilterDim, type Filters } from "@/lib/graph/filters";
import { graphColor } from "@/lib/graph/graph-colors";
import { SPACING } from "@/lib/graph/spacing";
import { LAYOUT_MODES, DEGREE_MODES, type HubOptions } from "@/lib/graph/hubs";
import type { SearchMode } from "@/lib/types";
import { cn, number } from "@/lib/utils";

type Props = {
  /** The graph picker: the one interface a replacement picker implements (components/graph-picker). */
  picker: GraphPickerProps;
  /** The spine view's control: None or a spine per open graph with declared spines. */
  spine?: SpinePanel;
  /** Shown nodes per open graph, for colour by graph. */
  graphCounts: Map<string, number>;
  graph: ViewGraph | null;
  version: number;
  theme: Theme;
  colourBy: ColourBy;
  groups: Groups | null;
  groupLabels: string[];
  statuses: Map<string, number>;
  grouping: Grouping;
  onGrouping: (g: Grouping) => void;
  resolution: number;
  onResolution: (r: number) => void;
  /** The layout's spacing (plan 198): a change lays the graph out again from where it is. */
  spacing: number;
  onSpacing: (s: number) => void;
  layout: HubOptions;
  onLayout: (value: HubOptions) => void;
  hubCount: number;
  hubFallback: boolean;
  louvainMs: number | null;
  /** A Louvain run for the current settings is still on its way. */
  clustering: boolean;
  query: string;
  onQuery: (q: string) => void;
  mode: SearchMode;
  onMode: (m: SearchMode) => void;
  search: SearchState;
  place: (id: string) => Placement;
  onPickNode: (id: string) => void;
  onReveal: (id: string) => void;
  filters: Filters;
  chips: ChipCounts | null;
  onChip: (dim: FilterDim, key: string) => void;
  /** A relation chip's state (plan 150): quiet, normal or off, and a plain-line swatch when symmetric. */
  relationLook?: (relation: string) => { mode: EdgeMode; symmetric: boolean };
  /** A relation chip was set away from its default: Reset filters puts it back. */
  modesChanged?: boolean;
  /** Chips an agent filter just changed ("dim:key"): they pulse once. */
  pulse?: ReadonlySet<string>;
  onResetFilters: () => void;
  /** Shadow connections' legend row and control (ws009 M1c), under the relation chips. */
  shadows?: ShadowLegend | null;
  visible: { provenance: boolean; retired: boolean };
  onVisible: (v: { provenance: boolean; retired: boolean }) => void;
  hiddenCounts: { provenance: number; retired: number };
  types: [string, number][];
  searchRef: React.RefObject<HTMLInputElement | null>;
};

export type Grouping = "louvain" | "parent" | "graph";
const GROUPING = [
  ["louvain", "Louvain"],
  ["parent", "Parent"],
  ["graph", "Graph"],
] as const;
const MODES = [
  ["title", "Title"],
  ["hybrid", "Search"],
] as const;
const PLACEHOLDER: Record<SearchMode, string> = { title: "Title, typos ok  ( / )", hybrid: "Words or meaning  ( / )" };
/** Clusters listed in the legend; the rest are summed in one line. */
const LEGEND_GROUPS = 14;

const SAMPLE: Record<StatusKind, string> = { open: "open", tentative: "proposed", settled: "agreed", blocked: "blocked", other: "x" };

export function Sidebar({ searchRef, ...p }: Props) {
  return (
    <nav className="fui-frame flex h-full w-[280px] shrink-0 flex-col overflow-y-auto border-r bg-card/60" aria-label="Graphs and filters">
      <div className="space-y-2 px-4 pt-4 pb-3">
        <GraphPicker {...p.picker} />
      </div>
      {p.spine ? <SpinePicker {...p.spine} /> : null}
      <div className="space-y-2 border-t px-4 py-3" data-testid="layout-controls">
        <label className="block space-y-1 text-sm">
          <span className="section-head">Layout</span>
          <select aria-label="Layout" value={p.layout.mode} onChange={(e) => {
            const mode = LAYOUT_MODES.find(([key]) => key === e.target.value)?.[0];
            if (mode) p.onLayout({ ...p.layout, mode });
          }} className="w-full rounded-md border bg-background px-2 py-1.5 focus-visible:outline-2 focus-visible:outline-ring" disabled={!p.graph}>
            {LAYOUT_MODES.map(([key, label]) => <option key={key} value={key}>{label}</option>)}
          </select>
        </label>
        {p.layout.mode === "degree" ? <label className="block space-y-1 text-sm">
          <span>Rank hubs by</span>
          <select aria-label="Rank hubs by" value={p.layout.degree} onChange={(e) => {
            const degree = DEGREE_MODES.find(([key]) => key === e.target.value)?.[0];
            if (degree) p.onLayout({ ...p.layout, degree });
          }} className="w-full rounded-md border bg-background px-2 py-1.5 focus-visible:outline-2 focus-visible:outline-ring">
            {DEGREE_MODES.map(([key, label]) => <option key={key} value={key}>{label}</option>)}
          </select>
        </label> : null}
        {p.layout.mode === "declared" ? <p className="text-xs text-muted-foreground">Inferred part_of preview; not yet verified by fill rules.</p> : null}
        {p.layout.mode !== "normal" ? <label className="flex items-center justify-between gap-2 text-sm">
          <span>Hubs</span>
          <input type="number" aria-label="Hubs" min={1} max={100} step={1} value={p.layout.count} onChange={(e) => {
            const count = e.currentTarget.valueAsNumber;
            if (Number.isInteger(count) && count >= 1 && count <= 100) p.onLayout({ ...p.layout, count });
          }} className="w-20 rounded-md border bg-background px-2 py-1 text-right tabular-nums focus-visible:outline-2 focus-visible:outline-ring" />
        </label> : null}
        {p.layout.mode !== "normal" ? <p className="text-xs text-muted-foreground" role="status">
          {p.hubCount > 0 ? `${number.format(p.hubCount)} hub${p.hubCount === 1 ? "" : "s"}` : p.hubFallback ? "No eligible degree hubs." : `No ${p.layout.mode === "declared" ? "part_of" : p.layout.mode} hubs available; using Normal.`}
        </p> : null}
        {p.hubFallback ? <p className="text-xs text-muted-foreground" role="status">No similarity data; using Degree hubs (most connections).</p> : null}
      </div>

      <div className="space-y-2 border-t px-4 py-3" data-testid="search">
        <div className="flex items-center justify-between gap-2">
          <h2 className="section-head">Find</h2>
          <Segmented label="Search mode" testId="search-mode" value={p.mode} options={MODES} onChange={p.onMode} />
        </div>
        <div className="relative">
          <Search className="pointer-events-none absolute top-2.5 left-2.5 size-4 text-muted-foreground" />
          <Input
            ref={searchRef}
            value={p.query}
            onChange={(e) => p.onQuery(e.target.value)}
            onKeyDown={(e) => {
              // Esc clears the box first; a second Esc leaves it (and the app clears focus, then the selection).
              if (e.key === "Escape" && p.query !== "") {
                e.preventDefault();
                e.nativeEvent.stopImmediatePropagation();
                p.onQuery("");
              }
            }}
            placeholder={PLACEHOLDER[p.mode]}
            className="pl-8"
            maxLength={200}
            disabled={!p.graph}
            data-testid="find"
            aria-label={p.mode === "title" ? "Find a node by title (umx node find)" : "Search words and meaning (umx search)"}
          />
          {p.search.loading ? <Loader2 className="absolute top-2.5 right-2.5 size-4 animate-spin text-muted-foreground" /> : null}
        </div>
        {p.search.error ? (
          <div className="rounded-md border border-destructive/40 bg-destructive/5 px-2 py-1 text-xs" data-testid="search-error">
            <span className="font-mono text-destructive">{p.search.error.code}</span>: {p.search.error.message}
          </div>
        ) : null}
        {p.search.result ? (
          <div className="max-h-72 overflow-y-auto">
            <HitList result={p.search.result} theme={p.theme} place={p.place} onPick={p.onPickNode} onReveal={p.onReveal} testId="search-results" />
          </div>
        ) : null}
      </div>

      <div className="space-y-2.5 border-t px-4 py-3">
        <h2 className="section-head">Show</h2>
        <label className="flex items-center justify-between gap-2 text-sm">
          <span>
            Provenance <span className="text-muted-foreground tabular-nums">({number.format(p.hiddenCounts.provenance)})</span>
          </span>
          <Switch checked={p.visible.provenance} onCheckedChange={(v) => p.onVisible({ ...p.visible, provenance: v })} />
        </label>
        <label className="flex items-center justify-between gap-2 text-sm">
          <span>
            Out of play <span className="text-muted-foreground tabular-nums">({number.format(p.hiddenCounts.retired)})</span>
          </span>
          <Switch checked={p.visible.retired} onCheckedChange={(v) => p.onVisible({ ...p.visible, retired: v })} />
        </label>
      </div>

      <div className="space-y-2 border-t px-4 py-3" data-testid="filters">
        <div className="flex items-center justify-between gap-2">
          <h2 className="section-head">Filters</h2>
          <button
            type="button"
            onClick={p.onResetFilters}
            disabled={!anyOff(p.filters) && !p.modesChanged}
            className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground disabled:opacity-40"
            data-testid="reset-filters"
          >
            <RotateCcw className="size-3" /> Reset filters
          </button>
        </div>
        {p.chips ? (
          <>
            <ChipGroup title="Relations" dim="relations" counts={p.chips.relations} off={p.filters.relations} onChip={p.onChip} pulse={p.pulse} look={p.relationLook} />
            {p.shadows ? <ShadowLegendRow {...p.shadows} /> : null}
            <ChipGroup title="Types" dim="types" counts={p.chips.types} off={p.filters.types} onChip={p.onChip} pulse={p.pulse} theme={p.theme} />
            <ChipGroup title="Statuses" dim="statuses" counts={p.chips.statuses} off={p.filters.statuses} onChip={p.onChip} pulse={p.pulse} />
          </>
        ) : null}
      </div>

      <div className="space-y-2.5 border-t px-4 py-3" data-testid="clusters">
        <h2 className="section-head">Clusters</h2>
        <div className="flex items-center justify-between gap-2 text-sm">
          <span>Group by</span>
          <Segmented label="Group by" testId="grouping" value={p.grouping} options={GROUPING} onChange={p.onGrouping} />
        </div>
        <label className="block space-y-1 text-sm">
          <span className="flex items-center justify-between">
            <span className={cn(p.grouping !== "louvain" && "text-muted-foreground")}>Resolution</span>
            <span className="text-xs text-muted-foreground tabular-nums">{p.resolution.toFixed(1)}</span>
          </span>
          <input
            type="range"
            min={0.2}
            max={3}
            step={0.1}
            value={p.resolution}
            disabled={(p.grouping !== "louvain" && p.layout.mode !== "similarity") || !p.graph}
            onChange={(e) => p.onResolution(Number(e.target.value))}
            className="w-full accent-primary disabled:opacity-50"
            aria-label="Louvain resolution"
            data-testid="resolution"
          />
        </label>
        <label className="block space-y-1 text-sm">
          <span className="flex items-center justify-between">
            <span>Spacing</span>
            <span className="text-xs text-muted-foreground tabular-nums">{p.spacing.toFixed(1)}</span>
          </span>
          <input
            type="range"
            min={SPACING.min}
            max={SPACING.max}
            step={SPACING.step}
            value={p.spacing}
            disabled={!p.graph}
            onChange={(e) => p.onSpacing(Number(e.target.value))}
            className="w-full accent-primary disabled:opacity-50"
            aria-label="Layout spacing"
            data-testid="spacing"
          />
        </label>
        <p className="text-xs text-muted-foreground tabular-nums" data-testid="cluster-stats">
          {p.clustering
            ? "Clustering…"
            : p.groups
              ? `${number.format(p.groups.sizes.filter((n) => n >= HULL_MIN).length)} groups of ${HULL_MIN}+` +
                (p.grouping === "louvain" && p.louvainMs !== null ? ` · Louvain ${number.format(p.louvainMs)} ms` : " · part_of trees")
              : ""}
        </p>
      </div>

      <div className="space-y-3 border-t px-4 py-3" data-testid="legend">
        {p.colourBy === "graph" || (p.colourBy === "cluster" && p.grouping === "graph") ? (
          <GraphLegend open={p.picker.open} counts={p.graphCounts} theme={p.theme} />
        ) : p.colourBy === "cluster" ? (
          <ClusterLegend groups={p.groups} labels={p.groupLabels} theme={p.theme} />
        ) : p.colourBy === "status" ? (
          <>
            <h2 className="section-head">Status</h2>
            <ul className="space-y-1">
              {STATUS_KINDS.map(([kind, label]) => (
                <li key={kind} className="flex items-center gap-2 text-sm">
                  <span aria-hidden className="inline-block size-2.5 shrink-0 rounded-full" style={{ background: statusColor(kind, p.theme) }} />
                  <span className="min-w-0 flex-1 truncate">{label}</span>
                  <span className="text-[11px] text-muted-foreground tabular-nums">{number.format(p.statuses.get(kind) ?? 0)}</span>
                </li>
              ))}
            </ul>
          </>
        ) : (
          <>
            <h2 className="section-head">Types</h2>
            <ul className="space-y-1">
              {p.types.map(([type, count]) => (
                <li key={type} className="flex items-center gap-2 text-sm">
                  <TypeDot type={type} theme={p.theme} />
                  <span className="min-w-0 flex-1 truncate">{type}</span>
                  <span className="text-[11px] text-muted-foreground tabular-nums">{number.format(count)}</span>
                </li>
              ))}
            </ul>
          </>
        )}
        <h2 className="section-head pt-1">Status ring</h2>
        <ul className="space-y-1">
          {STATUS_KINDS.map(([kind, label]) => {
            const look = statusLook(SAMPLE[kind], p.theme);
            return (
              <li key={kind} className="flex items-center gap-2 text-sm">
                <span
                  aria-hidden
                  className="inline-block size-3 rounded-full bg-muted-foreground/30"
                  style={{ boxShadow: `inset 0 0 0 ${Math.max(1, Math.round(look.ringSize * 8))}px ${look.ring}` }}
                />
                <span className="text-muted-foreground">{label}</span>
              </li>
            );
          })}
        </ul>
      </div>
    </nav>
  );
}

/** Colour by graph: one swatch per open graph, in its open order, with its nodes on screen. */
function GraphLegend({ open, counts, theme }: { open: readonly string[]; counts: Map<string, number>; theme: Theme }) {
  return (
    <>
      <h2 className="section-head">Graphs</h2>
      <ul className="space-y-1" data-testid="graph-legend">
        {open.map((alias, rank) => (
          <li key={alias} className="flex items-center gap-2 text-sm">
            <span aria-hidden className="inline-block size-2.5 shrink-0 rounded-full" style={{ background: graphColor(rank, theme) }} />
            <span className="min-w-0 flex-1 truncate">{alias}</span>
            <span className="text-[11px] text-muted-foreground tabular-nums">{number.format(counts.get(alias) ?? 0)}</span>
          </li>
        ))}
      </ul>
    </>
  );
}

function ClusterLegend({ groups, labels, theme }: { groups: Groups | null; labels: string[]; theme: Theme }) {
  if (!groups) return <h2 className="section-head">Clusters</h2>;
  const shown = groups.sizes.map((size, rank) => ({ size, rank })).filter((g) => g.size >= HULL_MIN);
  const rest = groups.sizes.reduce((sum, size) => sum + size, 0) - shown.slice(0, LEGEND_GROUPS).reduce((sum, g) => sum + g.size, 0);
  return (
    <>
      <h2 className="section-head">Clusters</h2>
      <ul className="space-y-1">
        {shown.slice(0, LEGEND_GROUPS).map(({ size, rank }) => (
          <li key={rank} className="flex items-center gap-2 text-sm">
            <span aria-hidden className="inline-block size-2.5 shrink-0 rounded-full" style={{ background: groupColor(rank, size, theme) }} />
            <span className="min-w-0 flex-1 truncate">{labels[rank]}</span>
            <span className="text-[11px] text-muted-foreground tabular-nums">{number.format(size)}</span>
          </li>
        ))}
      </ul>
      {rest > 0 ? (
        <p className="flex items-center gap-2 text-xs text-muted-foreground">
          <span aria-hidden className="inline-block size-2.5 shrink-0 rounded-full" style={{ background: groupColor(-1, 0, theme) }} />
          {number.format(rest)} in smaller groups
        </p>
      ) : null}
    </>
  );
}

function ChipGroup({
  title,
  dim,
  counts,
  off,
  onChip,
  theme,
  pulse,
  look,
}: {
  title: string;
  dim: FilterDim;
  counts: Map<string, number>;
  off: ReadonlySet<string>;
  onChip: (dim: FilterDim, key: string) => void;
  theme?: Theme;
  pulse?: ReadonlySet<string>;
  look?: (key: string) => { mode: EdgeMode; symmetric: boolean };
}) {
  const chips = chipList(counts, off);
  if (chips.length === 0) return null;
  return (
    <div className="space-y-1">
      <h3 className="text-[11px] text-muted-foreground">{title}</h3>
      <ul className="flex max-h-32 flex-wrap gap-1 overflow-y-auto" data-testid={`chips-${dim}`}>
        {chips.map(([key, count]) => {
          const on = !off.has(key);
          const edge = look?.(key);
          const pulsing = pulse?.has(`${dim}:${key}`) ?? false;
          return (
            <li key={key}>
              <button
                type="button"
                aria-pressed={on}
                onClick={() => onChip(dim, key)}
                data-key={key}
                data-count={count}
                data-pulse={pulsing || undefined}
                data-mode={edge?.mode}
                title={edge ? `${key}: ${edge.mode}. Click for ${edge.mode === "quiet" ? "normal" : edge.mode === "normal" ? "off" : "quiet"}` : on ? `Hide ${key}` : `Show ${key}`}
                className={cn(
                  "flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs transition-colors",
                  pulsing && "agent-pulse",
                  on ? "bg-card hover:bg-muted" : "border-dashed bg-transparent text-muted-foreground line-through decoration-muted-foreground/60 hover:bg-muted",
                )}
              >
                {edge ? <EdgeSwatch symmetric={edge.symmetric} mode={edge.mode} /> : null}
                {theme ? <TypeDot type={key} theme={theme} className={cn("size-2", !on && "opacity-40")} /> : null}
                <span className="max-w-[140px] truncate">{key}</span>
                <span className="text-[10px] text-muted-foreground tabular-nums">{number.format(count)}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/** A relation chip's swatch: an arrow (a plain line for symmetric relations), thin and faint when quiet. */
function EdgeSwatch({ symmetric, mode }: { symmetric: boolean; mode: EdgeMode }) {
  return (
    <svg aria-hidden width="14" height="8" viewBox="0 0 14 8" data-swatch={symmetric ? "line" : "arrow"} className={cn("shrink-0", mode === "normal" ? "text-foreground" : "text-muted-foreground opacity-60")}>
      <line x1="1" y1="4" x2={symmetric ? 13 : 9.5} y2="4" stroke="currentColor" strokeWidth={mode === "normal" ? 1.5 : 1} />
      {symmetric ? null : <path d="M9 1.2 L13.4 4 L9 6.8 Z" fill="currentColor" />}
    </svg>
  );
}
