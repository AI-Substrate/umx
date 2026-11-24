"use client";
import { ArrowLeft, ArrowRight, CircleAlert, ExternalLink, Loader2, Sparkles, X } from "lucide-react";
import { useMemo } from "react";
import { EntitySection, type EntityView } from "@/components/entity-section";
import { HitList, type Placement } from "@/components/hit-list";
import { NodeSpines, type NodeSpinesView } from "@/components/node-spines";
import { Segmented } from "@/components/segmented";
import { IdText, TextOrLink, Value } from "@/components/text";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { TypeDot } from "@/components/type-dot";
import { statusLook, type Theme } from "@/lib/graph/colors";
import { FORCED_LABELS_MAX } from "@/lib/graph/edges";
import type { Focus } from "@/lib/graph/focus";
import type { ApiError, Neighbour, NodeDetail, ResolvedNeighbour, SearchResult, StateNeighbour } from "@/lib/types";
import { TITLE_HINT } from "@/lib/secret-flags";
import { cn } from "@/lib/utils";

const STANDARD = new Set([
  "id", "type", "title", "status", "note", "evidence", "evidence_kinds", "status_history", "created", "updated",
  "created_by", "address", "graph",
]);

type Props = {
  open: boolean;
  detail: NodeDetail | null;
  error: ApiError | null;
  loading: boolean;
  theme: Theme;
  onClose: () => void;
  onPick: (id: string) => void;
  hiddenIds: (id: string) => boolean;
  /** Focus on this node: depth 0 is off, else 1 to 3 hops. */
  focus: { depth: number; focus: Focus; onDepth: (depth: number) => void };
  /** The node's similar_to list, read from the daemon when asked for. Optional so older call sites still render. */
  similar?: Similar;
  /** The node the panel showed was removed by a live change (plan 127): the panel says so until it is closed. */
  removed?: boolean;
  /** The graph the node is in (plan 133). */
  graph?: string | null;
  /** How neighbours map to the canvas: the key of one on it (any open graph), else null; and a graph's words. */
  links?: Links;
  /** The node's drawn edges past the FORCED_LABELS_MAX whose labels the canvas shows (plans 150, 198). */
  unlabelled?: number;
  /** The node's entity across the open graphs (ws009 M2), above Neighbours; null when it is in none. */
  entity?: EntityView | null;
  /** Its shadow connections (ws009 M1c): far node, graph and score, best first, and umx's notes for graphs not
   * compared; null while shadows are off. */
  shadows?: { rows: { key: string; title: string; graph: string; score: number }[]; notes: string[] } | null;
  /** How the node sits on each spine its graph declares, with toggles shared with the sidebar's Spine chips. */
  spines?: NodeSpinesView | null;
};

/** Where a neighbour is on the canvas (plan 133): its key when its graph is open, and how to name its graph. */
export type Links = { target: (n: Neighbour) => string | null; graphName: (n: Neighbour) => string };

export type Similar = {
  loading: boolean;
  result: SearchResult | null;
  error: ApiError | null;
  onAsk: () => void;
  place: (id: string) => Placement;
  onReveal: (id: string) => void;
};

function SimilarSection({ similar, theme, onPick }: { similar: Similar; theme: Theme; onPick: (id: string) => void }) {
  return (
    <Section title="Similar to this node">
      <SimilarBody similar={similar} theme={theme} onPick={onPick} />
    </Section>
  );
}

/** The node's own similar_to list, linked or not, on request (folded into the shadows' section while shadows are on,
 * ui-expert N3). */
function SimilarBody({ similar, theme, onPick }: { similar: Similar; theme: Theme; onPick: (id: string) => void }) {
  return (
    <>
      {similar.result ? (
        <HitList result={similar.result} theme={theme} place={similar.place} onPick={onPick} onReveal={similar.onReveal} testId="similar-results" />
      ) : (
        <Button variant="outline" size="sm" onClick={similar.onAsk} disabled={similar.loading} data-testid="similar-ask">
          {similar.loading ? <Loader2 className="animate-spin" /> : <Sparkles />}
          Find similar (similar_to)
        </Button>
      )}
      {similar.error ? (
        <p className="text-xs" data-testid="similar-error">
          <span className="font-mono text-destructive">{similar.error.code}</span>: {similar.error.message}
        </p>
      ) : null}
    </>
  );
}

const FOCUS = [
  [0, "Off"],
  [1, "1"],
  [2, "2"],
  [3, "3"],
] as const;


function StatusBadge({ status, theme }: { status: string; theme: Theme }) {
  const look = statusLook(status, theme);
  return (
    <Badge className="bg-card">
      <span aria-hidden className="inline-block size-2.5 rounded-full border-2" style={{ borderColor: look.ring }} />
      {status}
    </Badge>
  );
}

/** Console's panel while a node is read: a short fixed skeleton, still, in place of the spinner (ui-expert K1-N5). */
export function PanelSkeleton() {
  return (
    <div className="space-y-2 px-5" data-testid="panel-skeleton" aria-busy="true">
      <span className="sr-only">Reading the node…</span>
      {["w-2/3", "w-1/2", "w-5/6", "w-1/3"].map((w) => (
        <div key={w} className={cn("h-3 rounded-sm bg-muted", w)} />
      ))}
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-2 border-t px-5 py-4">
      <h3 className="section-head">{title}</h3>
      {children}
    </section>
  );
}

type Evidence = { source?: unknown; kind?: unknown; fidelity?: unknown; selector?: unknown; redacted?: unknown };

/** One evidence pointer: a node's, or a status move's (ws008 TM6, plan 192). A redacted entry (`remove --redact`,
 * F204) has no source and shows "(redacted)" as `list --removed` does, never another field in its place. */
function EvidenceRow({ item }: { item: Evidence }) {
  const redacted = item.redacted === true;
  const selector = redacted ? null : (item.selector as Record<string, unknown> | null | undefined);
  const quote = selector && typeof selector.quote === "string" ? selector.quote : null;
  return (
    <li className="space-y-1 rounded-md border bg-background/60 px-3 py-2 text-sm">
      <div className="font-mono text-xs break-all">
        {redacted ? <span className="text-muted-foreground">(redacted)</span> : <TextOrLink value={String(item.source ?? "")} />}
      </div>
      {quote ? <blockquote className="border-l-2 pl-2 text-muted-foreground italic">{quote}</blockquote> : null}
      {selector && !quote ? <Value value={selector} /> : null}
      <div className="flex gap-1.5 text-[11px] text-muted-foreground">
        {item.kind ? <span>{String(item.kind)}</span> : null}
        {item.fidelity ? <span>· {String(item.fidelity)}</span> : null}
      </div>
    </li>
  );
}

function graphLabel(n: Neighbour): string {
  return n.graphAlias ?? n.graph;
}

/** Before plan 133 a neighbour was on the canvas only when it was in the one graph on screen. */
const ONE_GRAPH: Links = { target: (n) => (n.kind === "node" && n.local ? n.id : null), graphName: graphLabel };

/** A neighbour umx could not resolve: its state and id, nothing else (there is no type, title or status). */
function StateRow({ n, links }: { n: StateNeighbour; links: Links }) {
  return (
    <div className="flex items-start gap-2 rounded-md px-2 py-1.5 text-sm" data-testid="neighbour-state">
      <CircleAlert className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden />
      <div className="min-w-0 flex-1">
        <div className="font-mono text-xs break-all">{n.id}</div>
        <div className="text-[11px] text-muted-foreground">
          {n.state} · {n.local ? "this graph" : `graph ${links.graphName(n)}`}
          {n.candidates.length > 0 ? ` · ${n.candidates.length} candidates` : ""}
        </div>
      </div>
    </div>
  );
}

/** A resolved node in a graph that is not open: labelled, not navigable (open its graph too to follow it). */
function ExternalRow({ n, theme, links }: { n: ResolvedNeighbour; theme: Theme; links: Links }) {
  return (
    <div className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm text-muted-foreground" data-testid="neighbour-external" title="In a graph that is not open: open it too to follow this link.">
      <TypeDot type={n.type} theme={theme} />
      <span className="min-w-0 flex-1 truncate">{n.title}</span>
      <span className="flex items-center gap-1 text-[11px]">
        <ExternalLink className="size-3" aria-hidden />
        other graph {links.graphName(n)}
      </span>
    </div>
  );
}

export function NeighbourRow({
  n,
  theme,
  onPick,
  hiddenIds,
  links = ONE_GRAPH,
}: {
  n: Neighbour;
  theme: Theme;
  onPick: (id: string) => void;
  hiddenIds: (id: string) => boolean;
  links?: Links;
}) {
  if (n.kind === "state") return <StateRow n={n} links={links} />;
  const key = links.target(n);
  if (key === null) return <ExternalRow n={n} theme={theme} links={links} />;
  return (
    <button
      type="button"
      onClick={() => onPick(key)}
      className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-muted"
      data-testid={n.local ? "neighbour-local" : "neighbour-open"}
    >
      <TypeDot type={n.type} theme={theme} />
      <span className="min-w-0 flex-1 truncate">{n.title}</span>
      {hiddenIds(key) ? <span className="text-[10px] text-muted-foreground">hidden</span> : null}
      {n.local ? null : (
        <span className="flex items-center gap-1 text-[11px] text-muted-foreground" data-testid="neighbour-graph">
          <ExternalLink className="size-3" aria-hidden />
          {links.graphName(n)}
        </span>
      )}
      <span className="text-[11px] text-muted-foreground">{n.type}</span>
    </button>
  );
}

export function NeighbourList({
  rows,
  theme,
  onPick,
  hiddenIds,
  links,
}: {
  rows: Neighbour[];
  theme: Theme;
  onPick: (id: string) => void;
  hiddenIds: (id: string) => boolean;
  links?: Links;
}) {
  const groups = useMemo(() => {
    const map = new Map<string, Neighbour[]>();
    for (const row of rows) {
      const key = `${row.way}\u0000${row.relation}`;
      const list = map.get(key);
      if (list) list.push(row);
      else map.set(key, [row]);
    }
    return [...map.entries()];
  }, [rows]);
  if (rows.length === 0) return <p className="text-sm text-muted-foreground">No neighbours.</p>;
  return (
    <div className="space-y-3">
      {groups.map(([key, list]) => {
        const [way, relation] = key.split("\u0000");
        return (
          <div key={key} className="space-y-1">
            <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
              {way === "in" ? <ArrowLeft className="size-3" /> : <ArrowRight className="size-3" />}
              <span className="font-mono">{relation}</span>
              <span className="tabular-nums">({list.length})</span>
            </div>
            <ul>
              {list.map((n) => (
                <li key={`${n.edge}:${n.graph}:${n.id}`}>
                  <NeighbourRow n={n} theme={theme} onPick={onPick} hiddenIds={hiddenIds} links={links} />
                </li>
              ))}
            </ul>
          </div>
        );
      })}
    </div>
  );
}

export function NodePanel({ open, detail, error, loading, theme, onClose, onPick, hiddenIds, focus, similar, removed = false, graph = null, links, unlabelled = 0, entity = null, shadows = null, spines = null }: Props) {
  const node = removed ? undefined : detail?.node;
  const fields = node ? Object.entries(node).filter(([k]) => !STANDARD.has(k)) : [];
  const evidence = Array.isArray(node?.evidence) ? (node.evidence as Evidence[]) : [];
  const history = Array.isArray(node?.status_history) ? (node.status_history as Record<string, unknown>[]) : [];
  return (
    <aside
      aria-label="Node"
      aria-hidden={!open}
      data-testid="node-panel"
      className={cn(
        "fui-frame absolute top-0 right-0 bottom-0 z-20 flex w-[420px] max-w-[90vw] flex-col border-l bg-card shadow-xl transition-transform duration-200 ease-out fui:duration-300 motion-reduce:transition-none",
        open ? "translate-x-0" : "pointer-events-none translate-x-full",
      )}
    >
      {/* The FUI skins draw the brackets in again on each selection (bracket-in). */}
      {open ? <span key={node ? String(node.id) : "none"} className="fui-brackets bracket-in" aria-hidden /> : null}
      <div className="flex items-start gap-2 px-5 pt-4 pb-3">
        <div className="min-w-0 flex-1 space-y-2">
          {node ? (
            <div className="flex flex-wrap items-center gap-1.5">
              <Badge className="bg-card">
                <TypeDot type={String(node.type)} theme={theme} />
                {String(node.type)}
              </Badge>
              <StatusBadge status={String(node.status)} theme={theme} />
              {graph ? (
                <Badge className="bg-card font-mono" data-testid="node-graph">
                  {graph}
                </Badge>
              ) : null}
            </div>
          ) : null}
          {removed ? (
            <div className="space-y-1" data-testid="node-removed">
              <h2 className="text-lg leading-snug font-semibold">This node was removed</h2>
              <p className="text-sm text-muted-foreground">A change through umx took it out of the graph.</p>
            </div>
          ) : (
            <h2 className="text-lg leading-snug font-semibold break-words" data-testid="node-title">
              {node ? String(node.title) : loading ? "Loading…" : ""}
            </h2>
          )}
          {node && detail?.titleHidden && !removed ? (
            <p className="text-xs text-muted-foreground" data-testid="title-hidden">
              {TITLE_HINT}
            </p>
          ) : null}
          {node?.address ? <IdText value={String(node.address)} className="text-muted-foreground" /> : null}
          {unlabelled > 0 && !removed ? (
            <p className="text-xs text-muted-foreground tabular-nums" data-testid="unlabelled" title={`The canvas labels ${FORCED_LABELS_MAX} of this node's edges`}>
              +{unlabelled} {unlabelled === 1 ? "edge" : "edges"}
            </p>
          ) : null}
          <div className={cn("flex items-center gap-2 pt-1", removed && "hidden")}>
            <span className="text-xs text-muted-foreground">Focus hops</span>
            <Segmented<number> label="Focus hops" testId="focus" value={focus.depth} options={FOCUS} onChange={focus.onDepth} />
            {focus.focus.state === "on" ? (
              <span className="text-xs text-muted-foreground tabular-nums" data-testid="focus-count">
                {focus.focus.count} {focus.focus.count === 1 ? "node" : "nodes"} · Esc clears
              </span>
            ) : focus.focus.state === "unavailable" ? (
              <span className="text-xs text-muted-foreground" data-testid="focus-paused">
                Paused: this node is hidden
              </span>
            ) : null}
          </div>
        </div>
        <Button variant="ghost" size="icon-sm" aria-label="Close" onClick={onClose}>
          <X />
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto pb-6">
        {loading && !node && !removed ? (
          theme === "console" ? (
            <PanelSkeleton />
          ) : (
            <div className="flex items-center gap-2 px-5 text-sm text-muted-foreground">
              <Loader2 className="size-4 animate-spin" /> Reading the node…
            </div>
          )
        ) : null}
        {error && !removed ? (
          <div className="mx-5 rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-sm">
            <p className="font-mono text-xs text-destructive">{error.code}</p>
            <p>{error.message}</p>
            {error.next ? <p className="font-mono text-xs text-muted-foreground">next: {error.next}</p> : null}
          </div>
        ) : null}
        {node ? (
          <>
            {typeof node.note === "string" && node.note !== "" ? (
              <Section title="Note">
                <p className="text-sm leading-relaxed whitespace-pre-wrap break-words">{node.note}</p>
              </Section>
            ) : null}
            {spines ? (
              <Section title="Spines">
                <NodeSpines {...spines} />
              </Section>
            ) : null}
            {fields.length > 0 ? (
              <Section title={entity?.merged ? "This node's fields" : "Fields"}>
                <dl className="grid grid-cols-[minmax(80px,auto)_1fr] gap-x-3 gap-y-2 text-sm">
                  {fields.map(([k, v]) => (
                    <div key={k} className="contents">
                      <dt className="font-mono text-xs text-muted-foreground pt-0.5">{k}</dt>
                      <dd className="min-w-0">
                        <Value value={v} />
                      </dd>
                    </div>
                  ))}
                </dl>
              </Section>
            ) : null}
            {evidence.length > 0 ? (
              <Section title={`Evidence (${evidence.length})`}>
                <ul className="space-y-2">
                  {evidence.map((item, i) => (
                    <EvidenceRow key={i} item={item} />
                  ))}
                </ul>
              </Section>
            ) : null}
            {entity ? <EntitySection {...entity} /> : null}
            <Section title={`Neighbours (${detail!.neighbours.length}${detail!.neighboursTruncated ? "+" : ""})`}>
              <NeighbourList rows={detail!.neighbours} theme={theme} onPick={onPick} hiddenIds={hiddenIds} links={links} />
            </Section>
            {shadows ? (
              <Section title={`Similar, not linked (in open graphs) (${shadows.rows.length})`}>
                <ul className="space-y-1" data-testid="shadow-list">
                  {shadows.rows.map((r) => (
                    <li key={r.key} className="flex items-center gap-2 text-sm" data-key={r.key}>
                      <button type="button" className="min-w-0 flex-1 truncate text-left hover:underline" onClick={() => onPick(r.key)}>
                        {r.title}
                      </button>
                      <Badge className="bg-card font-mono text-[10px]">{r.graph}</Badge>
                      <span className="font-mono text-xs text-muted-foreground tabular-nums">{r.score.toFixed(2)}</span>
                    </li>
                  ))}
                </ul>
                {shadows.rows.length === 0 ? <p className="text-xs text-muted-foreground">None above the model&apos;s floor.</p> : null}
                {shadows.notes.map((n) => (
                  <p key={n} className="text-xs text-muted-foreground" data-testid="shadow-panel-note">
                    {n}
                  </p>
                ))}
                {similar ? (
                  <div className="space-y-2 pt-1">
                    <p className="text-[11px] text-muted-foreground">Its own similar_to list, linked or not:</p>
                    <SimilarBody similar={similar} theme={theme} onPick={onPick} />
                  </div>
                ) : null}
              </Section>
            ) : similar ? (
              <SimilarSection similar={similar} theme={theme} onPick={onPick} />
            ) : null}
            {history.length > 0 ? (
              <Section title="Status history">
                <ol className="space-y-1.5 text-sm">
                  {history.map((h, i) => (
                    <li key={i} className="space-y-0.5">
                      <div>
                        <span className="font-mono text-xs">{String(h.from ?? "")}</span> →{" "}
                        <span className="font-mono text-xs">{String(h.to ?? "")}</span>
                        <span className="ml-2 text-[11px] text-muted-foreground">{String(h.at ?? h.when ?? "")}</span>
                      </div>
                      {h.reason ? <p className="text-muted-foreground">{String(h.reason)}</p> : null}
                      {/* The move's evidence, as the Evidence section draws a node's; its `in` anchor is not drawn (plan 192). */}
                      {h.evidence && typeof h.evidence === "object" ? (
                        <ul data-testid="move-evidence">
                          <EvidenceRow item={h.evidence as Evidence} />
                        </ul>
                      ) : null}
                    </li>
                  ))}
                </ol>
              </Section>
            ) : null}
            <Section title="Record">
              <dl className="grid grid-cols-[80px_1fr] gap-y-1 text-xs">
                <dt className="text-muted-foreground">id</dt>
                <dd className="min-w-0">
                  <IdText value={String(node.id)} />
                </dd>
                <dt className="text-muted-foreground">created</dt>
                <dd>{String(node.created ?? "")}{node.created_by ? ` by ${String(node.created_by)}` : ""}</dd>
                <dt className="text-muted-foreground">updated</dt>
                <dd>{String(node.updated ?? "")}</dd>
              </dl>
            </Section>
          </>
        ) : null}
      </div>
    </aside>
  );
}
