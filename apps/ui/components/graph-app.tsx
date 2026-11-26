"use client";
import { AlertTriangle, Info, Loader2, Radio, Waypoints, X } from "lucide-react";
import dynamic from "next/dynamic";
import { useTheme } from "next-themes";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { CanvasPalette, GroupInfo } from "@/components/graph-canvas";
import { AgentStrip, type OpenBox, type StripEntry } from "@/components/agent-strip";
import { NodePanel } from "@/components/node-panel";
import type { NodeSpinesView } from "@/components/node-spines";
import { SelectionShare } from "@/components/selection-share";
import { Segmented } from "@/components/segmented";
import { hintItems, Hints, type Placement } from "@/components/hit-list";
import { Sidebar, type Grouping } from "@/components/sidebar";
import { useGraphSet } from "@/components/use-graph-set";
import type { LeftOut } from "@/lib/graph-set";
import { ThemeToggle } from "@/components/theme";
import { TooltipProvider } from "@/components/ui/tooltip";
import { type Nudge, useLayout } from "@/components/use-layout";
import { type LiveHooks, useAgent } from "@/components/use-agent";
import { Raiser } from "@/lib/raise";
import { countText, heldCount, heldTotal, type HoldNote, type LiveMode, liveMode, MODE_KEY, namedHeld, noteFor } from "@/lib/hold";
import { mergeResults, useSearch } from "@/components/use-search";
import { useSelectionShare } from "@/components/use-selection";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { api, ApiFailure, asIssue, onRenamed, post, renamedOf } from "@/lib/api";
import { VIEW_HEADER } from "@/lib/guard";
import { composeRenames, idsWithPending, movedSelection, type Rename, renameKey, renameNotice, renamesLanded, renameTarget } from "@/lib/rename";
import { applyRows, applySnapshot, changeText, type Extent, extentOf, goneAfterRead, type Outcome, qualifyRows, readChange, remapGroups, reopen, snapAfter } from "@/lib/graph/apply";
import { liveAll, type LiveCursor, type TabChange, toRead } from "@/lib/live";
import type { RowsReply } from "@/lib/rows";
import {
  applyLook,
  applyVisibility,
  buildGraph,
  chipCounts,
  type ColourBy,
  counts,
  edgeMask,
  edgeShown,
  type ExternalCount,
  externalCount,
  neighbourLinks,
  type Omitted,
  omittedAfter,
  omittedOf,
  type OpenGraphs,
  graphGroups,
  graphOverlap,
  indexGraph,
  openByGraphId,
  shownMask,
  type ViewGraph,
  type Visibility,
} from "@/lib/graph/build";
import { type IndexGraph, parentGroups } from "@/lib/graph/clusters";
import { hash, HULL_MIN, skinOf, type Theme, themeOf } from "@/lib/graph/colors";
import type { ReadoutValues } from "@/components/readouts";
import { cycleRelation, type RelationModes, relationMode, relationSymmetric, symmetricByGraph } from "@/lib/graph/edges";
import { effectiveFocus } from "@/lib/graph/fui";
import { graphColor } from "@/lib/graph/graph-colors";
import { aliasOfKey, qid, setKey, unq } from "@/lib/graph/keys";
import { SPACING, SPACING_KEY, spacingOf } from "@/lib/graph/spacing";
import { HUB_DEFAULTS, type HubOptions, type SimilarityLink } from "@/lib/graph/hubs";
import { isLaunch, keptSpinePicks, LOOK_DEFAULTS, OPEN_KEY, readLook, readOpen, readSpinePicks, reopenGraphs, SPINES_KEY, type ViewLook, writeLook } from "@/lib/view-url";
import { type FilterDim, hiddenReasons, NO_FILTERS, reasonText, reveal, toggleChip, toggled } from "@/lib/graph/filters";
import { FOCUS_OFF, focusDepths, focusOf } from "@/lib/graph/focus";
import { activeNotes, agentOff, cameraAsk, cameraMove, Gesture, type CameraAsk, chipDiff, decide, describe as describeEvent, LEDGER0, type Ledger, keepPersonRelations, openIgnored, openLanded, openMayMove, openPlan, openTail, personSets, raiseTitle, readEach, takeBatch } from "@/lib/agent";
import type { AgentEvent } from "@/lib/umx/socket";
import type { CameraState } from "sigma/types";
import { type Launch, type LaunchDeps, Launches, type Listed, listForLaunch, takeLaunch, watchLaunch } from "@/lib/launch";
import { applyMerge, coveredOf, entitiesOf, entityOf, homeOf, mergedPlaces, mergeRecords, NO_ENTITIES, throughLinks, worldOf } from "@/lib/graph/entities";
import type { EntityReply } from "@/lib/entity-read";
import { entityScope, offerMembers, replyFor, scopeOfAsk } from "@/lib/entity-scope";
import { allShadows, farEnd, forScope, HOVER_TOP, nodeShadows, SELECTED_TOP, SHADOW_ROWS_MAX, type ShadowMode, shadowScope, shownBy, skippedLegend, skippedPanel } from "@/lib/graph/shadows";
import { NO_SHADOWS, ShadowReads, type ShadowState } from "@/lib/shadow-reads";
import type { ShadowReply } from "@/lib/shadows-read";
import type { ShadowLegend } from "@/components/shadow-legend";

/** How long after a live change shadows are read again (an edit's vectors follow it, ws009 M1c). */
const SHADOW_SETTLE_MS = 10_000;
import type { EntityView } from "@/components/entity-section";
import { addMany, closePanel, escape, noticeAfterPrune, noticeShown, type RemovalNotice, removeFromSelection, MAX_SELECTION, noSelection, pickOne, type Selection, toggleOne } from "@/lib/selection";
import { type SimilarState, similarAsked, similarFor, similarReplied } from "@/lib/similar";
import type { ApiError, GraphSummary, NodeDetail, ScopeSummary, SearchMode, SearchResult, Snapshot, SpineInfo, SpinePlaces } from "@/lib/types";
import { nodeOnSpines, type Place, shownSpine, SPREAD, type SpineAxis, spineLayout, type SpineRead, spineReadsDue, type SpineState, toggleSpine } from "@/lib/graph/spine-layout";
import { cn, number } from "@/lib/utils";

const GraphCanvas = dynamic(() => import("@/components/graph-canvas"), { ssr: false });

const COLOUR_BY = [
  ["type", "Type"],
  ["cluster", "Cluster"],
  ["status", "Status"],
  ["graph", "Graph"],
] as const;

function readPalette(theme: Theme): CanvasPalette {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(name).trim();
  return {
    theme,
    skin: skinOf(theme),
    ink: v("--canvas-ink"),
    structure: v("--muted-foreground"),
    halo: v("--canvas-halo"),
    canvas: v("--canvas"),
    edge: v("--canvas-edge"),
    edgeStructure: v("--canvas-edge-structure"),
    edgeFocus: v("--canvas-edge-focus"),
    dim: v("--canvas-dim"),
    edgeDim: v("--canvas-edge-dim"),
    accent: v("--primary"),
    agent: v("--canvas-agent"),
    font: getComputedStyle(document.body).fontFamily,
    mono: `${v("--font-geist-mono") || "ui-monospace"}, ui-monospace, monospace`,
    chrome: v("--chrome"),
    chromeHi: v("--chrome-hi"),
    chromeText: v("--chrome-text"),
    grid: v("--grid"),
    glowSelect: v("--glow-select"),
    glowAgent: v("--glow-agent"),
    glowLive: v("--glow-live"),
  };
}

type Auth = "checking" | "ready";

function onVisibility(changed: () => void) {
  document.addEventListener("visibilitychange", changed);
  return () => document.removeEventListener("visibilitychange", changed);
}


/** Resolves once the first layout of `graph` is done, so a live change never renumbers nodes under it. */
function whenLaid(laid: { graph: ViewGraph | null; wake: Set<() => void> }, graph: ViewGraph, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const check = () => {
      if (signal.aborted) {
        laid.wake.delete(check);
        reject(new Error("aborted"));
      } else if (laid.graph === graph) {
        laid.wake.delete(check);
        resolve();
      }
    };
    laid.wake.add(check);
    signal.addEventListener("abort", check, { once: true });
    check();
  });
}

/** Why some edges are not on the canvas: an end in a graph that is not open, or a broken end (review N6;
 * plan 133 draws an external edge once both its graphs are open). */
function omittedNote(o: ExternalCount, notLoaded: number): string {
  const parts = [
    o.notOpen > 0 ? `${number.format(o.notOpen)} into graphs not open` : "",
    o.broken > 0 ? `${number.format(o.broken)} broken` : "",
    notLoaded > 0 ? `${number.format(notLoaded)} to nodes not loaded` : "",
  ].filter(Boolean);
  return parts.length > 0 ? ` · not drawn: ${parts.join(", ")}` : "";
}

/** Graphs a scope or "all" left out, counted and never named (d-20). */
function leftOutNote(l: LeftOut | null): string {
  if (!l) return "";
  const parts = [
    l.secured > 0 ? `${l.secured} secured graph${l.secured === 1 ? "" : "s"} left out` : "",
    l.unavailable > 0 ? `${l.unavailable} not opened` : "",
  ].filter(Boolean);
  return parts.join(", ");
}

/** What is on the canvas: one snapshot per open graph, merged into one graphology graph (plan 133). */
type View = {
  graph: ViewGraph;
  ids: string[];
  index: IndexGraph;
  seed: number;
  /** The open graphs' snapshots, in open order; the live feed keeps each one's totals current. */
  snaps: Snapshot[];
  /** The open graphs, as snaps; their set key; each one's rank (open order); their aliases by graph id (null for an
   * id open in several copies). */
  aliases: string[];
  key: string;
  ranks: Map<string, number>;
  open: OpenGraphs;
  dropped: number;
  /** Per graph, the edges it stores that are not drawn, and why; kept true by every live change (review B7). */
  omitted: Map<string, Omitted>;
  timing: { fetchMs: number; buildMs: number };
  /** The last live change's new nodes, for the layout's nudge (plan 127). */
  nudge: Nudge | null;
  /** One more for each live change applied. */
  data: number;
  /** Live changes applied, per graph (alias), so a read of one graph waits only for that graph's changes. */
  changes?: ReadonlyMap<string, number>;
  /** An agent's open (ws009 M1b1): the nodes kept where they were in the layout, and the frame and camera the new
   * canvas takes from the one before (or, for an Undo, from before the open). */
  keep?: Map<string, { x: number; y: number }>;
  frame?: Extent | null;
  camera?: CameraState | null;
};

/** What follows an agent's open once the new graphs are laid out: fit the camera to them, or offer Go. `asked`: the
 * person chose it (Open), so the fit needs no second look. */
type Then = { kind: "fit" | "go"; asked: boolean; graphs: string[]; tail: string };
type Positions = Map<string, { x: number; y: number }>;
/** An open set reached from the view on screen (ws009 M1b1), for the loading effect: the set it asked for, the frame
 * and camera the new canvas keeps, and for an Undo the positions the graphs it brings back had. */
/** The open graphs as the top bar names them: joined up to 3, else counted. */
function graphNames(aliases: readonly string[]): string {
  if (aliases.length === 0) return "no graph";
  return aliases.length > 3 ? `${aliases.length} graphs` : aliases.join(" + ");
}

/** `renames`: a rename being followed (plan 215), former alias to live alias, so the selection keeps its nodes. */
type Merge = { wanted: string; frame: Extent; camera: CameraState | null; positions: Positions | null; then: Then | null; renames?: ReadonlyMap<string, string> };


const NO_CHANGES: ReadonlyMap<string, number> = new Map();
const NO_SIMILARITIES: readonly SimilarityLink[] = [];

function spineState(r: SpineRead | undefined): SpineState {
  return !r || r.loading ? "loading" : r.failed ? "failed" : r.gone ? "gone" : r.supported ? "ready" : "unsupported";
}

const total = (snaps: readonly Snapshot[], k: "totalNodes" | "totalEdges") => snaps.reduce((n, s) => n + s[k], 0);

export function GraphApp() {
  const { resolvedTheme } = useTheme();
  const theme: Theme = themeOf(resolvedTheme);
  const [auth, setAuth] = useState<Auth>("checking");
  const [graphs, setGraphs] = useState<GraphSummary[] | null>(null);
  const [scopes, setScopes] = useState<ScopeSummary[] | null>(null);
  // The graphs asked for (plan 133), as their set key: the order they were opened in is their colour rank.
  // The named graphs, what a scope or "all" left out, and the include-secured toggle (components/use-graph-set).
  const set = useGraphSet();
  const wanted = set.key;
  const { launch: launchSet, open: openSet, settle: settleSet } = set;
  const [hintsHidden, setHintsHidden] = useState<string | null>(null);
  const [view, setView] = useState<View | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ApiError | null>(null);
  const [notice, setNotice] = useState<{ issue: ApiError; sticky: boolean } | null>(null);
  // The person's selection (lib/selection): every node in it has the accent ring and is published to agents; the
  // panel and focus follow its primary node.
  const [selection, setSelection] = useState<Selection>(() => noSelection(null));
  const selected = selection.primary;
  const selectionSet = useMemo(() => new Set(selection.ids), [selection.ids]);
  const [detail, setDetail] = useState<NodeDetail | null>(null);
  const [detailError, setDetailError] = useState<ApiError | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [jump, setJump] = useState<{ id: string; n: number } | null>(null);
  const [focusDepth, setFocusDepth] = useState(0);
  const [visible, setVisible] = useState<Visibility>({ provenance: false, retired: false });
  const [colourBy, setColourBy] = useState<ColourBy>("type");
  // Merge entities (ws009 M2) starts on.
  const [mergeEntities, setMergeEntities] = useState<boolean>(LOOK_DEFAULTS.merge);
  // The spine each open graph shows, where the person picked one (null: none); the rest show their main spine.
  const [spinePicks, setSpinePicks] = useState<ReadonlyMap<string, string | null>>(new Map());
  // null: graph when more than one graph is open, else Louvain, until the person picks one.
  const [groupingPick, setGrouping] = useState<Grouping | null>(null);
  const [resolution, setResolution] = useState(1);
  // The Spacing slider (plan 198), kept by number only, as the theme is; read once the page runs.
  const [spacing, setSpacingState] = useState<number>(SPACING.default);
  // The spine view's Spread slider, kept in the fragment with the rest of the look.
  const [spread, setSpread] = useState<number>(SPREAD.default);
  const [layoutOptions, setLayoutOptions] = useState<HubOptions>(HUB_DEFAULTS);
  const [shadowMode, setShadowMode] = useState<ShadowMode>("selected");
  const [shadowState, setShadowState] = useState<ShadowState>(NO_SHADOWS);
  useEffect(() => {
    let stored: string | null = null;
    try {
      stored = window.localStorage.getItem(SPACING_KEY);
    } catch {
      // No storage (a private window): the default.
    }
    setSpacingState(spacingOf(stored));
  }, []);
  const setSpacing = useCallback((s: number) => {
    setSpacingState(s);
    try {
      window.localStorage.setItem(SPACING_KEY, String(s));
    } catch {
      // Not kept: the spacing still holds for this tab.
    }
  }, []);
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<SearchMode>("title");
  // The chips, and which changes on them are still the agent's (lib/agent Ledger, review R224-3). Every change goes
  // through updateLedger, which keeps the ref and the state in step.
  const [ledger, setLedgerState] = useState<Ledger>(LEDGER0);
  const ledgerNow = useRef<Ledger>(LEDGER0);
  const updateLedger = useCallback((change: (l: Ledger) => Ledger) => {
    const next = change(ledgerNow.current);
    ledgerNow.current = next;
    setLedgerState(next);
  }, []);
  const filters = ledger.filters;
  // The look a reload keeps (lib/view-url): restored once from the fragment before the launch reads it, then written
  // back as it changes. Chip offs for types and statuses wait for the first open, which clears them.
  const lookRestored = useRef(false);
  const pendingOff = useRef<ViewLook["off"] | null>(null);
  useEffect(() => {
    if (!lookRestored.current) return;
    const look = writeLook({
      colourBy,
      grouping: groupingPick ?? undefined,
      resolution,
      spacing,
      spread,
      layout: layoutOptions.mode,
      degree: layoutOptions.degree,
      hubs: layoutOptions.count,
      merge: mergeEntities,
      provenance: visible.provenance,
      retired: visible.retired,
      off: { relations: [...filters.relations], types: [...filters.types], statuses: [...filters.statuses] },
    });
    // A launch link still in the address bar is takeLaunch's to read and drop.
    if (isLaunch(window.location.hash)) return;
    const url = window.location.pathname + window.location.search + (look ? `#${look}` : "");
    if (url !== window.location.pathname + window.location.search + window.location.hash) window.history.replaceState(null, "", url);
  }, [colourBy, groupingPick, resolution, spacing, spread, layoutOptions, mergeEntities, visible, filters]);
  // The open graphs, once the index is read: a reload in this tab opens them again (none, if the person cleared it).
  useEffect(() => {
    if (graphs === null || !lookRestored.current) return;
    try {
      const open = wanted ? wanted.split(",") : [];
      window.sessionStorage.setItem(OPEN_KEY, JSON.stringify(open));
      // Each open graph's spine pick, beside the open set; a closed graph's pick goes with it.
      window.sessionStorage.setItem(SPINES_KEY, keptSpinePicks(spinePicks, open));
    } catch {
      // No storage: a reload opens the first graph, as before.
    }
  }, [wanted, graphs, spinePicks]);
  // The relation chips' quiet or normal choices (plan 150), per relation across the open graphs. Kept when the open
  // graphs change, as the person's relation offs are (keepPersonRelations); Reset filters clears them.
  const [relationModes, setRelationModes] = useState<RelationModes>(() => new Map());
  const [similar, setSimilar] = useState<SimilarState>(null);
  const similarReq = useRef(0);
  const [jumped, setJumped] = useState<string | null>(null);
  const [palette, setPalette] = useState<CanvasPalette | null>(null);
  const [version, setVersion] = useState(0);
  const searchRef = useRef<HTMLInputElement>(null);
  // Live updates (plan 127): the last change's words for the indicator, a change that could not be applied, the
  // revision on screen, the node the panel showed when a change removed it, and a reread of the panel's node.
  const [liveNote, setLiveNote] = useState<{ text: string; n: number } | null>(null);
  // Each open graph whose last change could not be applied, until its next change applies (review B6).
  const [liveErrors, setLiveErrors] = useState<ReadonlyMap<string, ApiError>>(new Map());
  const [liveAt, setLiveAt] = useState<ReadonlyMap<string, LiveCursor>>(new Map());
  const [removedPanel, setRemovedPanel] = useState<RemovalNotice>(null);
  const [detailTick, setDetailTick] = useState(0);
  // The nodes the last live change touched, for the FUI skins' live-glow (K1).
  // `at` is when the change was applied (performance.now()), whatever the theme then, so a later switch to a FUI skin
  // shows only what is left of its glow (review B4).
  const [liveTouched, setLiveTouched] = useState<{ n: number; ids: string[]; at: number } | null>(null);
  // Live or Hold (plan 152): one switch for every open graph, kept by name only. `holding` is what the poll reads, set
  // with the state so a switch never waits for a render.
  const [liveModeNow, setLiveModeState] = useState<LiveMode>("live");
  const holding = useRef(false);
  useEffect(() => {
    let stored: string | null = null;
    try {
      stored = window.localStorage.getItem(MODE_KEY);
    } catch {
      // No storage (a private window): start live.
    }
    const m = liveMode(stored);
    holding.current = m === "hold";
    setLiveModeState(m);
  }, []);
  // What the strip says while held: an agent named a node only in held changes, or an apply took the selected node off.
  const [holdNote, setHoldNote] = useState<HoldNote | null>(null);

  // One numbering of launches for this tab, and the first launch read once (React may run the effect twice in dev,
  // and the fragment is gone after the first read).
  const [launches] = useState(() => new Launches());
  const initial = useRef<Promise<Launch | null> | null>(null);
  const mounted = useRef(true);
  // Agent opens (ws009 M1b1): the open set the loading effect reaches by merging, and the graphs being read for it.
  const merge = useRef<Merge | null>(null);
  // Renames being followed until their live graphs land (plan 215, review R404-1): former alias to live alias, in
  // memory only (graph-addressing expert rule 2).
  const pendingRenames = useRef<ReadonlyMap<string, string>>(new Map());
  // The open set the view is heading to: what is on screen plus what is still being read (review B1). Agent
  // opens fold into it; a person's own pick replaces it.
  const intended = useRef<string[]>([]);
  useEffect(() => {
    intended.current = wanted ? wanted.split(",") : [];
  }, [wanted]);
  // A graph whose read failed leaves the open set, so the person or an agent can open it again (review
  // follow-up, ws009 M1b2); the loading effect skips that one change of the set, since nothing is left to read.
  const settled = useRef<string | null>(null);
  // Reads the canvas's camera now (components/graph-canvas cameraOut).
  const cameraOut = useRef<(() => CameraState) | null>(null);
  const [opening, setOpening] = useState<string[]>([]);
  const [afterOpen, setAfterOpen] = useState<(Then & { graph: ViewGraph }) | null>(null);
  const liveAtNow = useRef(liveAt);
  useEffect(() => {
    liveAtNow.current = liveAt;
  }, [liveAt]);

  // Lists the graphs and opens some, unless a newer launch began meanwhile (review R222-2). `named` comes from a launch
  // link (`umx ui --graph <a,b>` adds g=<a,b>) whose launch code this run took: those graphs open, secured or not, since they
  // were named. `fresh` is a new link landing in an open tab: it switches only to graphs it names.
  const refreshGraphs = useCallback(
    (named: string[] | null, fresh: boolean, gen: number = launches.now()) =>
      listForLaunch(
        launches,
        gen,
        named,
        fresh,
        {
          list: async (named) =>
            api<Listed>(named && named.length > 0 ? `/api/graphs?follow=${named.map(encodeURIComponent).join(",")}` : "/api/graphs"),
          alive: () => mounted.current,
        },
        (r) => {
          if (r.kind === "failed") {
            setAuth("ready");
            setGraphs((g) => g ?? []);
            setError(asIssue(r.error));
            return;
          }
          setGraphs(r.graphs);
          setAuth("ready");
          launchSet(r.pick.aliases, r.fresh && r.pick.named);
          // A launch link that named a former alias opens the live graph (plan 215), said once, quietly.
          const refused = r.refused.find((x) => r.pick.missing.includes(x.alias));
          if (refused) {
            setNotice({ issue: { ...refused.issue, message: `${refused.alias}: ${refused.issue.message}` }, sticky: true });
          } else if (r.pick.missing.length > 0) {
            const names = r.pick.missing.join(", ");
            setNotice({ issue: { code: "ui_graph_not_listed", message: `umx does not know a graph called ${names}.`, next: "umx graph list --all" }, sticky: true });
          } else if (r.pick.blocked.length > 0) {
            const b = r.pick.blocked[0];
            setNotice({ issue: { code: "ui_graph_not_open", message: `${b.alias}: ${b.reason ?? "it cannot be opened."}`, next: b.fix }, sticky: true });
          } else if (r.renamed.length > 0) {
            setNotice({ issue: renameNotice(r.renamed[0]), sticky: false });
          }
        },
      ),
    [launches, launchSet],
  );

  // Hand a launch link's code to the server and drop the fragment from the address bar: at load, and again whenever a
  // new launch link lands in this tab (hashchange). Nothing that settles after unmount changes the view.
  useEffect(() => {
    mounted.current = true;
    const deps: LaunchDeps = {
      location: window.location,
      history: window.history,
      post: async (code) => {
        const res = await fetch("/api/launch", {
          method: "POST",
          headers: { "content-type": "application/json", [VIEW_HEADER]: "1" },
          body: JSON.stringify({ code }),
          credentials: "same-origin",
        });
        return res.ok;
      },
    };
    // A reload: the look from the fragment, and the graphs this tab had open (lib/view-url), read before the launch
    // drops the fragment. A launch link's own graphs win over the tab's.
    let kept: string[] | null = null;
    if (!lookRestored.current) {
      lookRestored.current = true;
      const look = readLook(window.location.hash);
      if (look.colourBy) setColourBy(look.colourBy);
      if (look.grouping) setGrouping(look.grouping);
      if (look.resolution !== undefined) setResolution(look.resolution);
      if (look.spacing !== undefined) setSpacingState(look.spacing);
      if (look.spread !== undefined) setSpread(look.spread);
      setLayoutOptions({ mode: look.layout ?? HUB_DEFAULTS.mode, degree: look.degree ?? HUB_DEFAULTS.degree, count: look.hubs ?? HUB_DEFAULTS.count });
      if (look.merge !== undefined) setMergeEntities(look.merge);
      if (look.provenance !== undefined || look.retired !== undefined) {
        setVisible((v) => ({ provenance: look.provenance ?? v.provenance, retired: look.retired ?? v.retired }));
      }
      if (look.off) {
        const off = look.off;
        pendingOff.current = off;
        updateLedger((l) => ({ ...l, filters: { ...l.filters, relations: new Set(off.relations) } }));
      }
      try {
        kept = readOpen(window.sessionStorage.getItem(OPEN_KEY));
        setSpinePicks(readSpinePicks(window.sessionStorage.getItem(SPINES_KEY)));
      } catch {
        // No storage: the first graph opens, as before.
      }
      if (kept !== null && kept.length === 0) openSet([]);
    }
    initial.current ??= takeLaunch(deps, launches);
    void initial.current.then((launch) => {
      if (mounted.current) void refreshGraphs(reopenGraphs(launch?.named ?? null, kept), false, launch?.gen ?? 0);
    });
    const stop = watchLaunch(window, deps, launches, (launch) => void refreshGraphs(launch.named, true, launch.gen));
    return () => {
      mounted.current = false;
      stop();
    };
  }, [launches, refreshGraphs, openSet, updateLedger]);

  // An on-demand load shows briefly; a link that named a graph the view cannot open stays until dismissed.
  useEffect(() => {
    if (!notice || notice.sticky) return;
    const t = setTimeout(() => setNotice(null), 6_000);
    return () => clearTimeout(t);
  }, [notice]);

  // The saved scopes, for the picker (plan 133); a failure leaves the list empty and the picker as it was.
  useEffect(() => {
    if (auth !== "ready") return;
    let live = true;
    api<{ scopes: ScopeSummary[] }>("/api/scopes")
      .then((r) => live && setScopes(r.scopes))
      .catch(() => live && setScopes([]));
    return () => {
      live = false;
    };
  }, [auth, graphs]);

  // Opening graphs (plan 133): each picked graph's snapshot as before, one after another, merged into one graph keyed
  // by alias and id. A graph that fails to open is named in a notice (the person picked it) and the rest open.
  useEffect(() => {
    if (!wanted) return;
    if (settled.current === wanted) {
      settled.current = null;
      return;
    }
    // What opened, once a failed read left graphs out: the set becomes it, quietly. None opened empties the set, so
    // the error stays on screen and picking the graph again reads it again (review B2).
    const settle = (landed: string[]) => {
      if (setKey(landed) === wanted) return;
      if (landed.length > 0) settled.current = setKey(landed);
      settleSet(landed);
    };
    const aliases = wanted.split(",");
    const m = merge.current;
    merge.current = null;
    const from = viewNow.current;
    if (m && m.wanted === wanted && from) {
      // An agent's open, or its Undo: what is on screen stays (positions, selection, filters, held revisions). Each
      // graph it adds is read on its own and joins as soon as it arrives (review B4); one this set no longer
      // wants never joins. A graph it closes goes with the first one that joins.
      let live = true;
      let base: View = from;
      let first = true;
      const arrived: Snapshot[] = [];
      const failed: { alias: string; issue: ApiError }[] = [];
      // The view as it is now: live changes give it new ids and counts on the same graph.
      const current = () => (viewNow.current && viewNow.current.graph === base.graph ? viewNow.current : base);
      // A rename being followed (plan 215) lands only once every live graph it moves to is here (review R404-1), so
      // a renamed graph's nodes never leave the screen or the selection before their replacement; `force` lands what
      // came once every read has settled.
      const land = (force = false) => {
        const v = current();
        const came = arrived.map((sn) => sn.graph);
        if (m.renames && !force && !renamesLanded(m.renames, aliases, v.aliases, came)) return;
        const target = openLanded(aliases, v.aliases, came);
        if (target.length === 0 || setKey(target) === v.key) return;
        const kept = v.snaps.filter((sn) => target.includes(sn.graph)).map((sn) => ({ ...sn, live: liveAtNow.current.get(sn.graph) ?? sn.live }));
        const added = arrived.filter((sn) => !v.aliases.includes(sn.graph) && target.includes(sn.graph));
        const r = reopen(v.graph, target, kept, added);
        // Kept where they are: every node on screen, and for an Undo the nodes it brings back where they were.
        const keep: Positions = new Map();
        r.graph.forEachNode((n) => {
          const at = v.graph.hasNode(n) ? v.graph.getNodeAttributes(n) : m.positions?.get(n);
          if (at) keep.set(n, homeOf(at));
        });
        // Until the layout places them, new nodes start right of what is kept, never on top of it.
        if (keep.size > 0) {
          let [x0, x1] = [Infinity, -Infinity];
          for (const at of keep.values()) [x0, x1] = [Math.min(x0, at.x), Math.max(x1, at.x)];
          const dx = (x1 - x0) * 1.15 + 1;
          for (const n of r.fresh) if (!keep.has(n)) r.graph.updateNodeAttribute(n, "x", (x) => (x ?? 0) + dx);
        }
        const frame = first ? m.frame : (v.frame ?? extentOf(v.graph));
        const camera = (first ? m.camera : null) ?? cameraOut.current?.() ?? null;
        const restore = first && m.camera !== null;
        first = false;
        const ranks = new Map(target.map((a, i) => [a, i]));
        const key = setKey(target);
        const has = (id: string | null) => id !== null && r.graph.hasNode(id);
        const next: View = {
          ...v,
          graph: r.graph,
          ids: r.ids,
          index: indexGraph(r.graph, ranks),
          seed: hash(key),
          snaps: r.snaps,
          aliases: target,
          key,
          ranks,
          open: r.open,
          omitted: r.omitted,
          nudge: null,
          data: 0,
          // Nothing kept on screen (--only to other graphs): a fresh canvas, framed as any new one. An Undo's camera
          // is always taken back (review B3).
          ...(keep.size > 0 || restore ? { keep, frame, camera } : { keep: undefined, frame: null, camera: null }),
        };
        base = next;
        // A rename being followed: the selection's nodes are the same nodes under the live alias.
        setSelection((sel) => movedSelection(sel, key, m.renames ?? new Map(), has));
        if (m.renames) pendingRenames.current = new Map([...pendingRenames.current].filter(([, to]) => !target.includes(to)));
        setLiveErrors((was) => new Map([...was].filter(([a]) => target.includes(a))));
        setLiveAt((was) => new Map([...was].filter(([a]) => target.includes(a))));
        setLiveTouched(null);
        setView(next);
      };
      const add = aliases.filter((a) => !from.aliases.includes(a));
      setOpening(add);
      if (add.length === 0) land();
      void readEach(
        add,
        (a) => api<Snapshot>(`/api/graphs/${encodeURIComponent(a)}/snapshot`),
        () => live,
        (a, snap) => {
          arrived.push(snap);
          setOpening((o) => o.filter((x) => x !== a));
          land();
        },
        (a, e) => {
          failed.push({ alias: a, issue: asIssue(e) });
          setOpening((o) => o.filter((x) => x !== a));
        },
      ).then(() => {
        if (!live) return;
        if (m.renames) land(true);
        if (failed.length > 0) {
          const f = failed[0];
          setNotice({ issue: { ...f.issue, message: `${failed.map((x) => x.alias).join(", ")} did not open: ${f.issue.message}` }, sticky: true });
        }
        const v = current();
        setAfterOpen(m.then ? { ...m.then, graphs: m.then.graphs.filter((g) => v.aliases.includes(g)), graph: v.graph } : null);
        if (failed.length > 0) settle(v.aliases);
      });
      return () => {
        live = false;
        setOpening([]);
      };
    }
    let live = true;
    setLoading(true);
    setError(null);
    setSelection(noSelection(null));
    // A set opened afresh follows no rename: a former alias may later name another graph (rule 2).
    pendingRenames.current = new Map();
    setFocusDepth(0);
    setQuery("");
    updateLedger(keepPersonRelations);
    const off = pendingOff.current;
    if (off) {
      pendingOff.current = null;
      updateLedger((l) => ({ ...l, filters: { relations: new Set(off.relations), types: new Set(off.types), statuses: new Set(off.statuses) } }));
    }
    setSimilar(null);
    setView(null);
    setLiveNote(null);
    setLiveErrors(new Map());
    setLiveAt(new Map());
    setRemovedPanel(null);
    setLiveTouched(null);
    const started = performance.now();
    void (async () => {
      const snaps: Snapshot[] = [];
      const failed: { alias: string; issue: ApiError }[] = [];
      // A graph renamed since it was named (plan 215): its live alias is read instead, once, and said once.
      let renamed: Rename | null = null;
      for (const a of aliases) {
        try {
          snaps.push(await api<Snapshot>(`/api/graphs/${encodeURIComponent(a)}/snapshot`));
        } catch (e) {
          const r = e instanceof ApiFailure ? renamedOf(e.issue) : null;
          if (r && r.from === a && !aliases.includes(r.to) && live) {
            try {
              const sn = await api<Snapshot>(`/api/graphs/${encodeURIComponent(r.to)}/snapshot`);
              if (sn.graphId === r.id) {
                if (!snaps.some((s) => s.graph === sn.graph)) snaps.push(sn);
                renamed ??= r;
              } else failed.push({ alias: a, issue: asIssue(e) });
            } catch (again) {
              failed.push({ alias: a, issue: asIssue(again) });
            }
          } else if (r && r.from === a) renamed ??= r;
          else failed.push({ alias: a, issue: asIssue(e) });
        }
        if (!live) return;
      }
      setLoading(false);
      if (snaps.length === 0) {
        const f = failed[0].issue;
        setError(aliases.length > 1 ? { ...f, message: `${failed.map((x) => x.alias).join(", ")} did not open: ${f.message}` } : f);
        settle([]);
        return;
      }
      const fetched = performance.now();
      // A named read loaded an idle graph (F72): say so, and list it as loaded now.
      // A hint since protocol 71 (ws002 X1); an older umx sent it as a warning.
      const onDemand = snaps.flatMap((sn) => [...(sn.warnings ?? []), ...(sn.hints ?? [])]).find((w) => w.code === "graph_loaded_on_demand");
      if (failed.length > 0) {
        const f = failed[0];
        setNotice({ issue: { ...f.issue, message: aliases.length > 1 ? `${failed.map((x) => x.alias).join(", ")} did not open: ${f.issue.message}` : f.issue.message }, sticky: true });
      } else if (renamed) setNotice({ issue: renameNotice(renamed), sticky: false });
      else if (onDemand) setNotice({ issue: onDemand, sticky: false });
      if (onDemand || renamed) void refreshGraphs(null, false);
      const built = buildGraph(snaps);
      const open = snaps.map((sn) => sn.graph);
      const ranks = new Map(open.map((a, i) => [a, i]));
      const key = setKey(open);
      setSelection(noSelection(key));
      setView({
        graph: built.graph,
        ids: built.ids,
        index: indexGraph(built.graph, ranks),
        seed: hash(key),
        snaps,
        aliases: open,
        key,
        ranks,
        open: openByGraphId(snaps),
        dropped: built.droppedEdges,
        omitted: built.omitted,
        timing: { fetchMs: Math.round(fetched - started), buildMs: Math.round(performance.now() - fetched) },
        nudge: null,
        data: 0,
      });
      if (failed.length > 0 || renamed) settle(open);
    })();
    return () => {
      live = false;
    };
  }, [wanted, refreshGraphs, updateLedger, settleSet]);

  // Opening from the picker: exactly these graphs; `left` when a scope or "all" chose them (counted, never named).
  // None clears the canvas: nothing is read, and what the last set showed goes with it.
  const openGraphs = useCallback(
    (aliases: string[], left?: LeftOut) => {
      setNotice(null);
      if (aliases.length === 0) {
        setError(null);
        setLoading(false);
        setSelection(noSelection(null));
        setFocusDepth(0);
        setQuery("");
        setSimilar(null);
        setView(null);
        setLiveNote(null);
        setLiveErrors(new Map());
        setLiveAt(new Map());
        setRemovedPanel(null);
        setLiveTouched(null);
      }
      openSet(aliases, left);
    },
    [openSet],
  );
  const [choosing, setChoosing] = useState(false);
  const shown = useMemo(() => view?.aliases ?? [], [view]);
  const shownKey = view?.key ?? null;
  const multi = shown.length > 1;
  const grouping: Grouping = groupingPick ?? (multi ? "graph" : "louvain");

  // One visibility rule (lib/graph/filters): the node mask and the edge mask feed the clusters, the canvas, focus,
  // the legend and the counts alike.
  const mask = useMemo(() => (view ? shownMask(view.graph, visible, filters) : null), [view, visible, filters]);
  const edges = useMemo(() => (view ? edgeMask(view.graph, filters) : null), [view, filters]);
  const layoutSimilarities = layoutOptions.mode !== "normal" && shadowMode !== "off" && !filters.relations.has("similar_to") && shadowState.data?.scope === shownKey
    ? shadowState.data.pairs : NO_SIMILARITIES;
  const { layout, louvain, hubs, nudged } = useLayout(view, mask, edges, resolution, spacing, layoutOptions, layoutSimilarities);
  const hubOwners = useMemo(() => new Map(hubs.owner), [hubs]);
  const parent = useMemo(() => (view && mask && edges ? parentGroups(view.index, mask, edges) : null), [view, mask, edges]);
  const chips = useMemo(() => (view ? chipCounts(view.graph, visible, filters) : null), [view, visible, filters]);
  // Louvain's groups are for the numbering it was asked with; after a live change they are read for the new one
  // until Louvain runs again.
  const louvainNow = useMemo(() => (louvain && view ? remapGroups(louvain.groups, louvain.ids, view.ids) : null), [louvain, view]);
  // Grouping by graph (plan 133): a hull per open graph, labelled with its alias, in its colour.
  const byGraph = useMemo(() => (view && mask ? graphGroups(view.graph, view.ranks, mask) : null), [view, mask]);
  const groups = grouping === "parent" ? parent : grouping === "graph" ? byGraph : louvainNow;
  const groupInfo = useMemo<GroupInfo | null>(
    () =>
      groups && view
        ? grouping === "graph"
          ? { sizes: groups.sizes, labels: view.aliases, heads: [], colors: view.aliases.map((_a, r) => graphColor(r, theme)) }
          : {
              sizes: groups.sizes,
              labels: groups.heads.map((h) => (h >= 0 && view.graph.hasNode(view.ids[h]) ? view.graph.getNodeAttribute(view.ids[h], "label") : "")),
              heads: groups.heads.filter((h, r) => h >= 0 && groups.sizes[r] >= HULL_MIN).map((h) => view.ids[h]),
            }
        : null,
    [groups, view, grouping, theme],
  );

  // Theme, toggles, colour mode and groups restyle the graph in place. Colour by cluster while grouping by graph is
  // colour by graph, so the nodes match their hulls.
  useEffect(() => {
    if (!view || !resolvedTheme) return;
    applyVisibility(view.graph, visible, filters);
    applyLook(view.graph, theme, colourBy === "cluster" && grouping === "graph" ? "graph" : colourBy, groups, view.ranks);
    // next-themes puts the theme's class on <html> in its own effect, which runs after this one: read the tokens
    // once the class is there, or the canvas keeps the last theme's colours.
    const read = () => setPalette(readPalette(theme));
    let raf = 0;
    if (document.documentElement.classList.contains(theme)) read();
    else raf = requestAnimationFrame(read);
    setVersion((v) => v + 1);
    return () => cancelAnimationFrame(raf);
  }, [view, theme, visible, filters, resolvedTheme, colourBy, groups, grouping]);

  // How much the open graphs' hulls overlap once laid out (plan 133 review N1), for the evidence driver.
  const overlap = useMemo(
    () => (view && layout.done && view.aliases.length > 1 ? graphOverlap(view.graph) : undefined),
    [view, layout.done, version], // eslint-disable-line react-hooks/exhaustive-deps
  );

  const focus = useMemo(
    () => (view ? focusOf(view.graph, selected, focusDepth, filters.relations) : FOCUS_OFF),
    [view, selected, focusDepth, filters, version], // eslint-disable-line react-hooks/exhaustive-deps
  );
  const focused = focus.state === "on" ? focus.depths : null;

  useEffect(() => {
    // Opening other graphs changes the set before the old selection is reset: never read a node from another set.
    if (!selected || !shownKey || selection.graph !== shownKey) {
      setDetail(null);
      return;
    }
    let live = true;
    setDetailLoading(true);
    setDetailError(null);
    const at = unq(selected);
    api<NodeDetail>(`/api/graphs/${encodeURIComponent(at.alias)}/nodes/${encodeURIComponent(at.id)}`)
      .then((d) => live && setDetail(d))
      .catch((e) => {
        if (!live) return;
        setDetail(null);
        setDetailError(asIssue(e));
      })
      .finally(() => live && setDetailLoading(false));
    return () => {
      live = false;
    };
  }, [selected, shownKey, selection.graph, detailTick]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
      if (e.key.toLowerCase() === "g" && !typing && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault();
        setChoosing(true);
      } else if ((e.key === "/" && !typing) || (e.key.toLowerCase() === "k" && (e.metaKey || e.ctrlKey))) {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      } else if (e.key === "Escape") {
        if (typing) (e.target as HTMLElement).blur();
        // Escape clears focus first, then the selection.
        const next = escape(focusDepth, selection);
        setFocusDepth(next.focusDepth);
        setSelection(next.selection);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [focusDepth, selection]);

  const selectionNow = useRef(selection);
  useEffect(() => {
    selectionNow.current = selection;
  }, [selection]);
  const pickNode = useCallback((id: string) => {
    setSelection((s) => pickOne(s, id));
    setJump((f) => ({ id, n: (f?.n ?? 0) + 1 }));
  }, []);
  // Canvas clicks: a click selects one node, shift-click adds or removes one, a click on the empty canvas clears.
  const onPick = useCallback((id: string, add: boolean) => setSelection((s) => (add ? toggleOne(s, id) : pickOne(s, id))), []);
  const onClear = useCallback(() => {
    setSelection((s) => noSelection(s.graph));
    setFocusDepth(0);
  }, []);
  const onClosePanel = useCallback(() => {
    setSelection(closePanel);
    setFocusDepth(0);
    setRemovedPanel(null);
  }, []);
  const onLasso = useCallback((ids: string[]) => {
    const { next, left } = addMany(selectionNow.current, ids);
    setSelection(next);
    if (left > 0) {
      setNotice({
        issue: { code: "ui_selection_capped", message: `A selection holds at most ${number.format(MAX_SELECTION)} nodes: ${number.format(left)} were not added.` },
        sticky: false,
      });
    }
  }, []);
  const share = useSelectionShare(selection, shown);

  const search = useSearch(shown, mode, query, view?.data ?? 0);

  /** Where a search or similar hit is: shown, hidden and why, or not in this (truncated) view. */
  const place = useCallback(
    (id: string): Placement => {
      if (!view?.graph.hasNode(id)) return { kind: "absent" };
      const a = view.graph.getNodeAttributes(id);
      const why = hiddenReasons(a, visible, filters);
      return why.length > 0 ? { kind: "hidden", reasons: why.map((r) => reasonText(a, r)) } : { kind: "shown" };
    },
    [view, visible, filters],
  );
  // A shown hit is selected and the camera moves to it; a hidden one is only selected, until "Show" reveals it.
  const pickHit = useCallback(
    (id: string) => {
      if (place(id).kind === "shown") pickNode(id);
      else setSelection((s) => pickOne(s, id));
    },
    [place, pickNode],
  );
  const revealHit = useCallback(
    (id: string) => {
      if (!view?.graph.hasNode(id)) return;
      const next = reveal(view.graph.getNodeAttributes(id), visible, filters);
      setVisible(next.visible);
      // Show takes over the chips it turns back on (review R224-3).
      updateLedger((l) => personSets(l, next.filters));
      pickNode(id);
    },
    [view, visible, filters, pickNode, updateLedger],
  );
  // A person's chip is theirs from now on: no agent Undo, clear or later filter turns it back (review R224-3).
  // A relation chip cycles quiet, normal, off (plan 150); its off is the same chip state as before.
  const onChip = useCallback(
    (dim: FilterDim, key: string) => {
      if (dim === "relations") {
        const off = ledgerNow.current.filters.relations;
        const next = cycleRelation(key, relationModes, off);
        setRelationModes(next.modes);
        if (next.off === off.has(key)) return;
      }
      updateLedger((l) => personSets(l, toggleChip(l.filters, dim, key), [`${dim}:${key}`]));
    },
    [updateLedger, relationModes],
  );

  // similar_to is a node's own list, in its own graph; its hits become canvas keys there.
  const askSimilar = useCallback(() => {
    if (!selected || !shownKey) return;
    const req = ++similarReq.current;
    const at = unq(selected);
    setSimilar(similarAsked(shownKey, selected, req));
    api<SearchResult>(`/api/graphs/${encodeURIComponent(at.alias)}/nodes/${encodeURIComponent(at.id)}/similar`)
      .then((result) => setSimilar((s) => similarReplied(s, req, { result: mergeResults([{ alias: at.alias, result }], false) })))
      .catch((e) => setSimilar((s) => similarReplied(s, req, { error: asIssue(e) })));
  }, [selected, shownKey]);
  const similarNow = similarFor(similar, shownKey, selected);

  // Neighbours on the canvas (lib/graph/build neighbourLinks): a link in any open graph, never a guessed copy.
  const securedIds = useMemo(() => new Set((graphs ?? []).filter((g) => g.secured).map((g) => g.id)), [graphs]);
  const links = useMemo(
    () => neighbourLinks(view?.open ?? new Map(), selected ? aliasOfKey(selected) : null, securedIds),
    [view, selected, securedIds],
  );

  const stats = useMemo(() => {
    if (!view) return null;
    const c = counts(view.graph);
    let shownNodes = 0;
    let shownEdges = 0;
    let toggledOff = 0;
    view.graph.forEachNode((_n, a) => {
      if (!a.hidden) shownNodes++;
      if (!toggled(a, visible)) toggledOff++;
    });
    view.graph.forEachEdge((e) => {
      if (edgeShown(view.graph, e, filters)) shownEdges++;
    });
    return {
      ...c,
      shownNodes,
      shownEdges,
      toggledOff,
      external: externalCount(view.graph, view.omitted.values()),
      types: [...c.types.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])),
    };
  }, [view, version, filters, visible]); // eslint-disable-line react-hooks/exhaustive-deps

  const hiddenIds = useCallback((id: string) => !view?.graph.hasNode(id) || view.graph.getNodeAttribute(id, "hidden"), [view]);

  // Live updates (plan 127): the poll that brings agent events also brings this graph's changes. Each is applied to
  // the graph on screen in place, after its first layout: the rows it touched are read by id (or, on overflow, the
  // whole snapshot again), added, changed or dropped; new nodes are nudged into place; the camera stays.
  const viewNow = useRef(view);
  useEffect(() => {
    viewNow.current = view;
  }, [view]);
  const laid = useRef<{ graph: ViewGraph | null; wake: Set<() => void> }>({ graph: null, wake: new Set() });
  useEffect(() => {
    laid.current.graph = view && layout.done ? view.graph : null;
    for (const wake of [...laid.current.wake]) wake();
  }, [view, layout.done]);
  const changes = useRef(0);
  // One graph's change at a time (plan 133): its touched rows are read in that graph (naming the other open graphs, so
  // its external edges between them come too), keyed by alias and applied; an overflow rereads that graph alone.
  const applyChange = useCallback<LiveHooks["apply"]>(
    async (alias: string, change: TabChange, epoch: string, how: "apply" | "reload", signal: AbortSignal) => {
      const v0 = viewNow.current;
      if (!v0 || !v0.aliases.includes(alias)) throw new Error("that graph is not on screen");
      await whenLaid(laid.current, v0.graph, signal);
      const v = viewNow.current;
      const same = () => viewNow.current?.graph === v0.graph && !signal.aborted;
      if (!v || !same()) throw new Error("the graphs on screen changed");
      const snap0 = v.snaps.find((sn) => sn.graph === alias)!;
      try {
        let outcome: Outcome;
        let snap: Snapshot;
        let omitted: Omitted;
        const to = change.kind === "overflow" ? change.rev : change.to;
        if (how === "reload" || change.kind === "overflow") {
          const fresh = await api<Snapshot>(`/api/graphs/${encodeURIComponent(alias)}/snapshot`, signal);
          if (!same()) throw new Error("the graphs on screen changed");
          outcome = applySnapshot(v.graph, fresh, v.open);
          omitted = omittedOf(v.graph, fresh, v.open);
          snap = fresh;
        } else {
          const want = toRead(change);
          const others = v.aliases.filter((a) => a !== alias);
          const rows = await readChange((body) => post<RowsReply>(`/api/graphs/${encodeURIComponent(alias)}/rows`, body, signal), want, others);
          if (!same()) throw new Error("the graphs on screen changed");
          const read = { nodes: rows.nodes, edges: [...rows.edges, ...(rows.external ?? []).map(([id, kind, , from, , to]): [string, string, string, string] => [id, kind, from, to])] };
          const gone = goneAfterRead(want, read, { nodes: change.nodes.removed, edges: change.edges.removed });
          outcome = applyRows(v.graph, qualifyRows(alias, rows, new Set(v.aliases)), {
            nodes: gone.nodes.map((id) => qid(alias, id)),
            edges: gone.edges.map((id) => qid(alias, id)),
          });
          const q = (id: string) => qid(alias, id);
          omitted = omittedAfter(viewNow.current?.omitted.get(alias), v.graph, want.edges.map(q), change.edges.removed.map(q));
          snap = snapAfter(snap0, outcome, rows, gone.edges);
        }
        const cursor = { epoch, rev: to };
        const n = ++changes.current;
        setView((prev) =>
          prev && prev.graph === v.graph
            ? {
                ...prev,
                snaps: prev.snaps.map((sn) => (sn.graph === alias ? snap : sn)),
                omitted: new Map(prev.omitted).set(alias, omitted),
                ids: outcome.ids,
                index: indexGraph(v.graph, prev.ranks),
                nudge: { n, fresh: outcome.fresh },
                data: prev.data + 1,
                changes: new Map(prev.changes).set(alias, (prev.changes?.get(alias) ?? 0) + 1),
              }
            : prev,
        );
        const removed = new Set(outcome.removed);
        const dropped = removeFromSelection(selectionNow, removed);
        const before = dropped.before;
        if (dropped.next !== before) {
          setSelection(dropped.next);
          // Composed in order, so another graph's pruning in the same poll carries the notice on (review B4 round 2).
          setRemovedPanel((notice) => noticeAfterPrune(notice, dropped));
          if (dropped.primaryGone && before.primary) {
            setFocusDepth(0);
            if (holding.current) setHoldNote({ set: setKey(v.aliases), kind: "removed" });
          }
        }
        if (before.primary && !removed.has(before.primary) && outcome.touched.has(before.primary)) setDetailTick((t) => t + 1);
        const text = changeText(outcome);
        if (text) setLiveNote({ text: multi ? `${alias}: ${text}` : text, n });
        setLiveTouched({ n, ids: [...outcome.touched].filter((id) => !removed.has(id)), at: performance.now() });
        setLiveErrors((m) => {
          if (!m.has(alias)) return m;
          const next = new Map(m);
          next.delete(alias);
          return next;
        });
        setLiveAt((m) => new Map(m).set(alias, cursor));
        return { cursor, removed: outcome.removed };
      } catch (e) {
        if (!signal.aborted) {
          const issue = asIssue(e);
          setLiveErrors((m) => new Map(m).set(alias, issue));
        }
        throw e;
      }
    },
    [multi],
  );
  const liveHooks = useRef<LiveHooks | null>(null);
  useEffect(() => {
    liveHooks.current = { start: (a) => viewNow.current?.snaps.find((sn) => sn.graph === a)?.live ?? { epoch: null, rev: 0 }, apply: applyChange, held: () => holding.current };
  }, [applyChange]);
  useEffect(() => {
    if (!liveNote) return;
    const t = setTimeout(() => setLiveNote(null), 5_000);
    return () => clearTimeout(t);
  }, [liveNote]);
  const removedId = noticeShown(removedPanel, selection);

  // Agent control (plan 114): what agents set on this graph, from the server's one UiWait loop. Presentation only:
  // agent events never touch the provenance and retired toggles, never select, never open a graph, and never move
  // the camera to or name a node the view does not show.
  const agent = useAgent(view ? shown : null, liveHooks);
  // umx stopped this view and said why (ui-expert N1): kept once seen, as the server is gone right after.
  const [ended, setEnded] = useState<ApiError | null>(null);
  useEffect(() => {
    if (agent.error?.code === "ui_ended") setEnded(agent.error);
  }, [agent.error]);
  // Hold (plan 152): each open graph's held changes (null: many, never shown as 0), only those with some.
  const heldCounts = useMemo(() => {
    const out = new Map<string, number | null>();
    if (liveModeNow !== "hold" || !view) return out;
    for (const a of view.aliases) {
      const c = heldCount(agent.held.get(a));
      if (c !== 0) out.set(a, c);
    }
    return out;
  }, [liveModeNow, view, agent.held]);
  const heldAll = liveModeNow === "hold" && view ? heldTotal(agent.held, view.aliases) : 0;
  const shownNote = liveModeNow === "hold" && view ? noteFor(holdNote, { set: setKey(view.aliases), aliases: view.aliases }) : null;
  const applyHeld = useCallback(
    (aliases: readonly string[]) => {
      setHoldNote(null);
      void agent.applyHeld(aliases);
    },
    [agent.applyHeld], // eslint-disable-line react-hooks/exhaustive-deps
  );
  // Hold to Live applies everything held (Apply all), then follows live; Live to Hold keeps what comes next.
  const switchMode = useCallback(
    (m: LiveMode) => {
      holding.current = m === "hold";
      setLiveModeState(m);
      try {
        window.localStorage.setItem(MODE_KEY, m);
      } catch {
        // Not kept: the mode still holds for this tab.
      }
      // Hold ends every automatic apply at once (an unfinished drain too); Live drains what is held as automatic work.
      if (m === "hold") agent.holdNow();
      else if (viewNow.current) {
        setHoldNote(null);
        void agent.drainHeld(viewNow.current.aliases);
      }
    },
    [agent.holdNow, agent.drainHeld], // eslint-disable-line react-hooks/exhaustive-deps
  );
  // Grouping by graph in Hold: each held graph's pill counts its changes.
  const groupShown = useMemo<GroupInfo | null>(
    () => (groupInfo && grouping === "graph" && heldCounts.size > 0 && view ? { ...groupInfo, held: view.aliases.map((a) => (heldCounts.has(a) ? countText(heldCounts.get(a)!) : null)) } : groupInfo),
    [groupInfo, grouping, heldCounts, view],
  );
  const [follow, setFollow] = useState(true);
  const [gesture] = useState(() => new Gesture());
  const tabVisible = useSyncExternalStore(onVisibility, () => document.visibilityState === "visible", () => true);
  const [fit, setFit] = useState<{ nodes: string[]; n: number } | null>(null);
  const [pendingAsk, setPendingAsk] = useState<CameraAsk | null>(null);
  const [prompt, setPrompt] = useState<CameraAsk | null>(null);
  const [pulse, setPulse] = useState<ReadonlySet<string>>(new Set());
  const handled = useRef(0);
  // The graph each agent filter on the chips came from: clear in another graph leaves it on (plan 133).
  const filterFrom = useRef<string | null>(null);
  // New graphs start with no agent filter on their chips and no camera request.
  useEffect(() => {
    handled.current = 0;
    filterFrom.current = null;
    setFit(null);
    setPendingAsk(null);
    setPrompt(null);
    setHoldNote(null);
    setOpenBox(null);
  }, [shownKey]);

  // Agent opens (ws009 M1b1). Each is acted on once per tab, whatever set is open when a poll replays it (an Undo
  // never brings it back); one that came before this tab opened is offered, never applied (lib/agent openPlan).
  const securedAliases = useMemo(() => new Set((graphs ?? []).filter((g) => g.secured).map((g) => g.alias)), [graphs]);
  const [viewSince] = useState(() => Date.now());
  const openSeen = useRef(new Set<string>());
  const [openBox, setOpenBox] = useState<OpenBox | null>(null);
  const [undoOpen, setUndoOpen] = useState<{ seq: number; after: string; prev: string[]; frame: Extent; camera: CameraState | null; positions: Positions } | null>(null);
  // Another open set reached from the one on screen: nothing on screen moves (the canvas keeps its frame, the layout
  // keeps the open graphs' positions); `undo` brings back a frame, camera and positions from before.
  const mergeOpen = useCallback(
    (target: string[], seq: number, then: Then | null, undo?: { frame: Extent; camera: CameraState | null; positions: Positions }) => {
      const v = viewNow.current;
      if (!v || setKey(target) === setKey(intended.current)) return;
      const frame = v.frame ?? extentOf(v.graph);
      merge.current = { wanted: setKey(target), frame: undo?.frame ?? frame, camera: undo?.camera ?? null, positions: undo?.positions ?? null, then };
      if (undo) setUndoOpen(null);
      else {
        const positions: Positions = new Map();
        v.graph.forEachNode((n, a) => {
          positions.set(n, homeOf(a));
        });
        setUndoOpen({ seq, after: setKey(target), prev: intended.current, frame, camera: cameraOut.current?.() ?? null, positions });
      }
      intended.current = target;
      setOpenBox(null);
      openSet(target);
    },
    [openSet],
  );
  // A graph open here was renamed (plan 215): any read of the view's server that named its former alias found the live
  // one (lib/snapshot liveAlias). Only a graph in the open set, and the same graph by id, switches (graph-addressing
  // expert rules 3 and 6): in one step, as an agent's open does, the live alias takes the former one's place (so its
  // colour), the screen keeps its nodes, positions, camera and selection, the live graph is read afresh and followed
  // from there (UiWait names the open set), and the publisher moves too. The mapping lives here, in memory only, and
  // only until the live graphs land: renames that come before then compose (review R404-1).
  const followRename = useCallback(
    (r: Rename) => {
      const v = viewNow.current;
      if (!v) return;
      const ids = idsWithPending(new Map(v.snaps.map((sn) => [sn.graph, sn.graphId])), pendingRenames.current);
      const target = renameTarget(intended.current, ids, r);
      if (!target) return;
      const renames = composeRenames(pendingRenames.current, r);
      pendingRenames.current = renames;
      const positions: Positions = new Map();
      v.graph.forEachNode((n, a) => {
        positions.set(renameKey(n, renames), homeOf(a));
      });
      merge.current = { wanted: setKey(target), frame: v.frame ?? extentOf(v.graph), camera: cameraOut.current?.() ?? null, positions, then: null, renames };
      intended.current = target;
      share.rename(r.from, r.to);
      // An Undo would reopen the former alias: it goes.
      setUndoOpen(null);
      setNotice({ issue: renameNotice(r), sticky: false });
      settleSet(target);
      // The picker lists the live alias, never the former one.
      void refreshGraphs(null, false);
    },
    [settleSet, share.rename, refreshGraphs], // eslint-disable-line react-hooks/exhaustive-deps
  );
  useEffect(() => onRenamed(followRename), [followRename]);
  const raiser = useRef<Raiser | null>(null);
  const onAgentOpen = useCallback(
    (e: AgentEvent) => {
      const c = e.command;
      if (!viewNow.current || c.kind !== "open") return;
      const raised = raiseTitle(e, securedAliases, Date.now());
      if (raised !== null) (raiser.current ??= new Raiser(document, window)).raise(raised);
      // Against the set the view is heading to, so two opens before the first lands both count (review B1).
      const plan = openPlan(e, intended.current, securedAliases, Date.now(), viewSince);
      if (plan.kind === "ignore") return;
      const tail = openTail(c.left_out, plan.leftOut);
      // Asked first: taking graphs away (--only), or an open from before this tab. Open reads the set as it is then.
      const accept = () => {
        const again = openPlan({ ...e, at_ms: Date.now() }, intended.current, securedAliases, Date.now(), 0);
        if (again.kind === "only") mergeOpen(again.graphs, e.seq, { kind: "fit", asked: true, graphs: again.graphs, tail: "" });
        else if (again.kind === "add") mergeOpen([...intended.current, ...again.add], e.seq, { kind: "fit", asked: true, graphs: again.add, tail: "" });
      };
      if (plan.kind === "only") {
        const closes = plan.closes.length > 0 ? ` (closes ${plan.closes.join(", ")})` : "";
        setOpenBox({ lead: "Agent wants to show only", graphs: plan.graphs.join(", "), tail: closes + tail, actions: [{ label: "Open", id: "open", run: accept }] });
        return;
      }
      if (plan.ask) {
        setOpenBox({ lead: "Agent wants to open", graphs: plan.add.join(", "), tail, actions: [{ label: "Open", id: "open", run: accept }] });
        return;
      }
      if (plan.add.length === 0) {
        setOpenBox({ lead: "Agent opened", graphs: "no graph", tail, actions: [] });
        return;
      }
      const idle = openMayMove({ now: Date.now(), follow, visible: tabVisible, busy: gesture.active, lastTouch: gesture.lastTouch });
      mergeOpen([...intended.current, ...plan.add], e.seq, { kind: idle ? "fit" : "go", asked: false, graphs: plan.add, tail });
    },
    [securedAliases, viewSince, mergeOpen, follow, tabVisible, gesture],
  );
  // Once the new graphs are laid out: the camera goes to them (following, idle), or the strip offers Go.
  useEffect(() => {
    if (!afterOpen || !view || view.graph !== afterOpen.graph || !layout.done) return;
    setAfterOpen(null);
    const nodes = view.graph.filterNodes((_n, a) => afterOpen.graphs.includes(a.graph) && !a.hidden);
    const go = () => setFit((f) => ({ nodes, n: (f?.n ?? 0) + 1 }));
    // Asked again now, after the reads and the layout: the person may have started using the canvas, turned Follow
    // off or left the tab meanwhile (review B2); then Go instead.
    const kind = afterOpen.kind === "fit" && !afterOpen.asked && !openMayMove({ now: Date.now(), follow, visible: tabVisible, busy: gesture.active, lastTouch: gesture.lastTouch }) ? "go" : afterOpen.kind;
    if (kind === "fit" && nodes.length > 0) go();
    if (kind === "go" || afterOpen.tail) {
      const run = () => {
        go();
        setOpenBox(null);
      };
      setOpenBox({ lead: "Agent opened", graphs: afterOpen.graphs.join(", "), tail: afterOpen.tail, actions: kind === "go" && nodes.length > 0 ? [{ label: "Go", id: "go", run }] : [] });
    }
  }, [afterOpen, view, layout.done, follow, tabVisible, gesture]);
  const undoAgentOpen = useCallback(() => {
    if (!undoOpen) return;
    mergeOpen(undoOpen.prev, undoOpen.seq, null, { frame: undoOpen.frame, camera: undoOpen.camera, positions: undoOpen.positions });
  }, [undoOpen, mergeOpen]);

  const shownNode = useCallback((id: string) => !!view?.graph.hasNode(id) && !view.graph.getNodeAttribute(id, "hidden"), [view]);
  const titleOf = useCallback((id: string) => (shownNode(id) ? String(view!.graph.getNodeAttribute(id, "label") ?? "") : null), [shownNode, view]);

  // Each batch once (lib/agent takeBatch): an agent filter replaces the last one on the chips (and pulses what
  // changed); clear acts within its own graph; the newest camera request waits for its decision below.
  useEffect(() => {
    const batch = agent.batch;
    if (!batch || !view || batch.n === handled.current) return;
    handled.current = batch.n;
    const out = takeBatch(batch.events, { ledger: ledgerNow.current, filterFrom: filterFrom.current, pending: pendingAsk, prompt });
    filterFrom.current = out.filterFrom;
    if (out.filterTouched) {
      updateLedger(() => out.ledger);
      setPulse(chipDiff(out.chipsBefore, out.ledger.filters));
    }
    setPendingAsk(out.pending);
    setPrompt(out.prompt);
    for (const e of batch.events) {
      const id = `${e.seq}:${e.at_ms}`;
      if (e.command.kind !== "open" || openSeen.current.has(id)) continue;
      openSeen.current.add(id);
      onAgentOpen(e);
    }
    // Held (plan 152): a command naming a node that is only in a graph's held changes is not applied by itself (an
    // agent never ends the hold); the strip says so and offers that graph's Apply.
    const inHeld = holding.current ? namedHeld(batch.events, agent.held, (key) => view.graph.hasNode(key)) : null;
    if (inHeld) setHoldNote({ set: setKey(view.aliases), kind: "pending", alias: inHeld });
  }, [agent.batch, view, updateLedger, pendingAsk, prompt, onAgentOpen]); // eslint-disable-line react-hooks/exhaustive-deps

  // A reset that rebuilt agent state with no filter (a new daemon epoch, events gone from its ring) takes the agent's
  // old filter back off the chips.
  useEffect(() => {
    if (agent.state.filter !== null || ledgerNow.current.agent.length === 0) return;
    updateLedger(agentOff);
  }, [agent.state.filter, updateLedger]);

  useEffect(() => {
    if (pulse.size === 0) return;
    const t = setTimeout(() => setPulse(new Set()), 1_400);
    return () => clearTimeout(t);
  }, [pulse]);

  // A fit with no nodes fits its own graph's shown nodes, not every graph on the canvas (review B5).
  // A fit for a graph none of whose nodes are shown leaves the camera alone (review B5 round 2): lib/agent cameraMove.
  const moveCamera = useCallback(
    (ask: CameraAsk) => {
      const v = viewNow.current;
      const nodes = v ? v.graph.mapNodes((key, a) => ({ key, graph: a.graph, hidden: a.hidden })) : [];
      const move = cameraMove(ask, nodes, v?.aliases.length ?? 1);
      if (move?.kind === "jump") setJump((j) => ({ id: move.id, n: (j?.n ?? 0) + 1 }));
      else if (move?.kind === "fit") setFit((f) => ({ nodes: move.nodes, n: (f?.n ?? 0) + 1 }));
    },
    [],
  );

  // Follow mode: move only when following, the tab is visible and the person has not touched the canvas for 5 s;
  // otherwise ask. A hidden tab holds the request until it is visible; a stale one, or one naming only nodes that are
  // not shown, never moves the camera.
  useEffect(() => {
    if (!pendingAsk || !view || !layout.done) return;
    const d = decide(pendingAsk, { now: Date.now(), follow, visible: tabVisible, lastTouch: gesture.lastTouch, busy: gesture.active, shown: shownNode });
    if (d === "defer") return;
    setPendingAsk(null);
    if (d === "move") moveCamera(pendingAsk);
    else if (d === "prompt") setPrompt(pendingAsk);
  }, [pendingAsk, view, layout.done, follow, tabVisible, shownNode, moveCamera, gesture]);

  const agentFocus = useMemo(() => {
    const f = agent.state.focus;
    if (!view || !f) return null;
    const depths = focusDepths(view.graph, f.node, f.hops, filters.relations);
    return depths.size > 0 ? depths : null;
  }, [view, agent.state.focus, filters, version]); // eslint-disable-line react-hooks/exhaustive-deps

  // The agent's notes on its active highlight and focus (review B2), never from history.
  const agentNotes = useMemo(() => activeNotes(agent.state), [agent.state]);
  // The focus the canvas draws: the person's, else the agent's (review B3). The card reads the same one.
  const shownFocus = useMemo(
    () => effectiveFocus(focused ? { depths: focused, hops: focusDepth } : null, agentFocus && agent.state.focus ? { depths: agentFocus, hops: agent.state.focus.hops } : null),
    [focused, focusDepth, agentFocus, agent.state.focus],
  );

  // Entities (ws009 M2, plan 165): built with umx's rule over the open graphs, from what is on the canvas, so a live
  // change, a Hold or an open rebuilds them with the view.
  const entityBase = useMemo(() => {
    if (!view) return null;
    const graphIds = new Map(view.snaps.map((sn) => [sn.graph, sn.graphId] as const));
    const world = worldOf(view.graph, graphIds, coveredOf(view.snaps), throughLinks(view.snaps, view.graph));
    return { graphIds, world, entities: entitiesOf(world) };
  }, [view]);
  const entities = entityBase?.entities ?? NO_ENTITIES;
  const entityBaseNow = useRef(entityBase);
  useEffect(() => {
    entityBaseNow.current = entityBase;
  }, [entityBase]);
  const walked = useMemo(
    () => (entityBase && selected && view?.graph.hasNode(selected) ? entityOf(entityBase.world, selected, entityBase.graphIds) : null),
    [entityBase, selected, view],
  );
  // umx's own answer for the selected node, for members in graphs that are not open and for the merged record. Asked
  // only for a node that can be a member (it holds keys or ends an identity edge), again after each live change.
  const candidate = !!(
    entityBase &&
    selected &&
    view?.graph.hasNode(selected) &&
    (view.graph.getNodeAttribute(selected, "keys")?.length || entityBase.world.links.has(`${entityBase.graphIds.get(aliasOfKey(selected))}/${unq(selected).id}`))
  );
  // A reply belongs to its node and the open set it was read over (review B3): one for another scope is never
  // shown or used, from the first render after the scope changes, so nothing from a closed graph outlives it.
  const scopeNow = entityScope(selected, view?.aliases ?? null, mergeEntities);
  const entityAsk = candidate && scopeNow && view ? `${scopeNow}|${view.data}` : null;
  const [entityReply, setEntityReply] = useState<{ ask: string; reply: EntityReply | null; error: ApiError | null; loading: boolean } | null>(null);
  useEffect(() => {
    if (!entityAsk) {
      setEntityReply(null);
      return;
    }
    let live = true;
    const [node, open, mergedAsked] = entityAsk.split("|");
    const at = unq(node);
    const others = open.split(",").filter((g) => g !== at.alias);
    // A live change rereads the same scope: its last reply stays until the new one comes.
    setEntityReply((was) => ({ ask: entityAsk, reply: replyFor(was, scopeOfAsk(entityAsk))?.reply ?? null, error: null, loading: true }));
    const url = `/api/graphs/${encodeURIComponent(at.alias)}/nodes/${encodeURIComponent(at.id)}/entity?with=${others.map(encodeURIComponent).join(",")}${mergedAsked === "1" ? "&merged=1" : ""}`;
    api<EntityReply>(url)
      .then((reply) => live && setEntityReply({ ask: entityAsk, reply, error: null, loading: false }))
      .catch((e) => live && setEntityReply({ ask: entityAsk, reply: null, error: asIssue(e), loading: false }));
    return () => {
      live = false;
    };
  }, [entityAsk]);
  const replyNow = replyFor(entityReply, scopeNow);
  const mergedRecord = useMemo(
    () => (mergeEntities && replyNow?.reply?.records ? mergeRecords(replyNow.reply.records.map((r) => ({ graph: r.graph, key: qid(r.graph, r.id), node: r.node }))) : null),
    [mergeEntities, replyNow],
  );
  // "Show across graphs" (design point 2): every member selected (the person's ring, K1 glow in Console), their
  // threads at emphasis width, the camera fitted to them and each member's hop 1 in focus. It lasts while the
  // selection still holds every member.
  const [across, setAcross] = useState<{ members: string[] } | null>(null);
  const showAcross = useCallback((members: string[], primary: string) => {
    setSelection((sel) => addMany(pickOne(sel, primary), members).next);
    setAcross({ members });
    setFit((f) => ({ nodes: members, n: (f?.n ?? 0) + 1 }));
  }, []);
  const acrossNow = across && selected && across.members.includes(selected) && across.members.every((m) => selectionSet.has(m)) ? across : null;
  const acrossSet = useMemo(() => (acrossNow ? new Set(acrossNow.members) : null), [acrossNow]);
  const acrossDepths = useMemo(() => {
    if (!acrossNow || !view) return null;
    const out = new Map<string, number>();
    for (const m of acrossNow.members) for (const [n, d] of focusDepths(view.graph, m, 1, filters.relations)) out.set(n, Math.min(d, out.get(n) ?? d));
    return out;
  }, [acrossNow, view, filters.relations]);
  // An agent's highlight of a member is offered on the strip, never done alone (M2; the verb is J1 or E3). The offer
  // names only open graphs and no key.
  // An offer holds only its node and the view it was made on (review B3): its members and graphs come from the
  // entities on screen now, it is gone once the open set changes, and Go reads them again when it runs.
  const [offer, setOffer] = useState<{ node: string; view: string } | null>(null);
  const offered = useRef(new Set<string>());
  const viewKey = view?.key ?? null;
  useEffect(() => {
    if (offer && offer.view !== viewKey) setOffer(null);
    // A member highlighted again after the agent cleared it is offered again.
    for (const id of offered.current) if (!agent.state.highlight.has(id)) offered.current.delete(id);
    for (const id of agent.state.highlight) {
      if (!entities.of.has(id) || offered.current.has(id) || viewKey === null) continue;
      offered.current.add(id);
      setOffer({ node: id, view: viewKey });
      return;
    }
  }, [agent.state.highlight, entities, viewKey, offer]);
  const offerShown = view ? offerMembers(offer, viewKey, entities, (k) => view.graph.hasNode(k)) : null;
  const offerBox: OpenBox | null =
    offer && offerShown
      ? {
          lead: "An agent highlighted one of ",
          graphs: `${offerShown.length} members of an entity in ${[...new Set(offerShown.map(aliasOfKey))].join(", ")}`,
          tail: ". Show the whole entity?",
          actions: [
            {
              label: "Go",
              id: "entity-go",
              run: () => {
                const v = viewNow.current;
                const members = v ? offerMembers(offer, v.key, entityBaseNow.current?.entities ?? NO_ENTITIES, (k) => v.graph.hasNode(k)) : null;
                if (members && members.length > 1) showAcross(members, offer.node);
                setOffer(null);
              },
            },
          ],
        }
      : null;
  // Spines (spine view): each open graph's declared spines, the one it shows (the person's pick, else its main or
  // first), and where umx places its nodes on it, read again after each live change to that graph. A list that could
  // not be read is "failed" (asked again when the graph is next opened).
  const [spineLists, setSpineLists] = useState<ReadonlyMap<string, SpineInfo[] | "failed">>(new Map());
  const spinesOf = useCallback((a: string) => {
    const l = spineLists.get(a);
    return Array.isArray(l) ? l : undefined;
  }, [spineLists]);
  const [spinePlaces, setSpinePlaces] = useState<ReadonlyMap<string, SpineRead>>(new Map());
  const spineAsked = useRef(new Set<string>());
  useEffect(() => {
    for (const a of shown) {
      if (spineAsked.current.has(a)) continue;
      spineAsked.current.add(a);
      api<{ spines: SpineInfo[] }>(`/api/graphs/${encodeURIComponent(a)}/spines`)
        .then((r) => setSpineLists((m) => new Map(m).set(a, r.spines)))
        .catch(() => {
          spineAsked.current.delete(a);
          setSpineLists((m) => new Map(m).set(a, "failed"));
        });
    }
  }, [shown]);
  const spineShown = useMemo(() => new Map(shown.map((a) => [a, shownSpine(spinesOf(a), spinePicks.has(a) ? spinePicks.get(a)! : undefined)])), [shown, spinesOf, spinePicks]);
  const changesNow = view?.changes ?? NO_CHANGES;
  // The node panel's graph: while it is open, every spine of that graph is read (once per spine, again after a live
  // change), so the panel can say how the node sits on each; the others are read only while shown.
  const panelGraph = selected ? aliasOfKey(selected) : null;
  const spineWanted = useMemo(() => {
    const out: [string, string][] = [];
    for (const [a, name] of spineShown) if (name) out.push([a, name]);
    if (panelGraph) for (const s of spinesOf(panelGraph) ?? []) if (spineShown.get(panelGraph) !== s.name) out.push([panelGraph, s.name]);
    return out;
  }, [spineShown, spinesOf, panelGraph]);
  useEffect(() => {
    for (const [a, name] of spineReadsDue(spineWanted, spinePlaces, changesNow)) {
      const key = `${a}/${name}`;
      const had = spinePlaces.get(key);
      const at = changesNow.get(a) ?? 0;
      setSpinePlaces((m) => new Map(m).set(key, { at, loading: true, places: had?.places ?? null, supported: had?.supported ?? true }));
      api<SpinePlaces>(`/api/graphs/${encodeURIComponent(a)}/spines/${encodeURIComponent(name)}`)
        .then((r) => {
          const places = new Map(r.places.map(([id, band, by, anchor]) => [id, { band, by, anchor: anchor === 1 }]));
          setSpinePlaces((m) => new Map(m).set(key, { at, loading: false, places, supported: r.supported, gone: r.gone }));
        })
        .catch(() => setSpinePlaces((m) => new Map(m).set(key, { at, loading: false, places: null, supported: true, failed: true })));
    }
  }, [spineWanted, spinePlaces, changesNow]);
  // The placements the canvas lays out now, by graph: only those read and supported.
  const spineNow = useMemo(() => {
    const out = new Map<string, ReadonlyMap<string, Place>>();
    for (const [a, name] of spineShown) {
      const r = name ? spinePlaces.get(`${a}/${name}`) : undefined;
      if (r?.places && r.supported && !r.gone) out.set(a, r.places);
    }
    return out;
  }, [spineShown, spinePlaces]);
  const viewGraph = view?.graph;
  const nodeSpines = useMemo(() => {
    if (!selected || !panelGraph) return null;
    const list = spineLists.get(panelGraph);
    const shownNow = spineShown.get(panelGraph) ?? null;
    const rows: NodeSpinesView["rows"] = list === "failed" ? "failed" : list
      ? nodeOnSpines(list, unq(selected).id, shownNow, (name) => {
          const r = spinePlaces.get(`${panelGraph}/${name}`);
          // A refresh after a live change keeps the last placements on show until the new ones land.
          return { state: r?.loading && r.places ? "ready" : spineState(r), places: r?.places ?? null };
        })
      : null;
    return {
      rows,
      anchor: (id: string) => {
        const key = qid(panelGraph, id);
        return { key, title: viewGraph?.hasNode(key) ? String(viewGraph.getNodeAttribute(key, "label")) : id };
      },
      onToggle: (name: string) => setSpinePicks((m) => new Map(m).set(panelGraph, toggleSpine(shownNow, name))),
      onPick: pickNode,
    };
  }, [selected, panelGraph, spineLists, spineShown, spinePlaces, viewGraph, pickNode]);
  const [spineAxes, setSpineAxes] = useState<SpineAxis[]>([]);
  const spineShape = useRef("");
  // The force layout's positions, kept while a spine is shown so none puts each node back where it was laid out.
  const spineBase = useRef<{ graph: ViewGraph; layout: unknown; nudged: number; at: Map<string, { x: number; y: number; clearance?: number }> } | null>(null);
  // Positions: back to the force layout, then along each graph's spine, then merged; off puts every member back.
  useEffect(() => {
    if (!view) return;
    const g = view.graph;
    applyMerge(g, NO_ENTITIES);
    const base = spineBase.current;
    if (base && base.graph === g && base.layout === layout) {
      if (base.nudged !== nudged) {
        for (const id of view.nudge?.fresh ?? []) if (g.hasNode(id)) {
          const a = g.getNodeAttributes(id);
          base.at.set(id, { x: a.x, y: a.y, clearance: a.clearance });
        }
        base.nudged = nudged;
      }
      g.updateEachNodeAttributes((k, a) => {
        const p = base.at.get(k);
        if (p) { [a.x, a.y] = [p.x, p.y]; a.clearance = p.clearance; }
        else base.at.set(k, { x: a.x, y: a.y, clearance: a.clearance });
        return a;
      });
    } else if (layout.done) {
      const at = new Map<string, { x: number; y: number; clearance?: number }>();
      g.forEachNode((k, a) => void at.set(k, { x: a.x, y: a.y, clearance: a.clearance }));
      spineBase.current = { graph: g, layout, nudged, at };
    }
    const axes: SpineAxis[] = [];
    if (layout.done && spineNow.size > 0) {
      // The lane takes the canvas's shape, so Fit fills it.
      const box = document.querySelector(".sigma-host")?.getBoundingClientRect();
      const aspect = box && box.width > 0 && box.height > 0 ? box.width / box.height : undefined;
      for (const [alias, places] of spineNow) {
        const nodes: { key: string; id: string; x: number; y: number }[] = [];
        g.forEachNode((k, a) => {
          if (a.graph === alias && !a.hidden) nodes.push({ key: k, id: unq(k).id, x: a.x, y: a.y });
        });
        const laid = spineLayout(alias, nodes, places, spread, aspect, hubOwners, spacing);
        g.updateEachNodeAttributes((k, a) => {
          const p = laid.at.get(k);
          if (p) { [a.x, a.y] = [p.x, p.y]; a.clearance = laid.clearance; }
          return a;
        });
        if (laid.axis) axes.push(laid.axis);
      }
    }
    setSpineAxes((old) => (old.length === 0 && axes.length === 0 ? old : axes));
    // A spine shown or put away moves every node: fit the camera to the new shape, once per change of spines.
    const shape = [...spineNow.keys()].sort().join(",");
    if (layout.done && shape !== spineShape.current) {
      spineShape.current = shape;
      setFit((f) => ({ nodes: [], n: (f?.n ?? 0) + 1 }));
    }
    applyMerge(g, mergeEntities && layout.done ? entities : NO_ENTITIES);
  }, [view, layout, mergeEntities, entities, visible, filters, nudged, spineNow, spread, hubOwners, spacing]);
  // Shadow connections (ws009 M1c, plan 169). ShadowReads owns the reads (review B2 to B4): the data belongs to
  // the open set it was read over, a new set drops it, Off stops and drops it, and only the newest read lands. Read
  // again on events: a new set, a live change or an Apply (10 s after the last), and the refresh button. Under Hold the
  // graphs holding changes are not read again, so shadows always match the applied canvas (review B3).
  const reads = useMemo(
    () => new ShadowReads((graphs, signal) => api<ShadowReply>(`/api/shadows?graphs=${graphs.map(encodeURIComponent).join(",")}`, signal), setShadowState),
    [],
  );
  useEffect(() => () => reads.dispose(), [reads]);
  const sScope = shadowScope(view?.key ?? null);
  const heldKey = [...heldCounts.keys()].sort().join(",");
  useEffect(() => {
    reads.update(sScope, view?.aliases ?? [], heldKey === "" ? [] : heldKey.split(","), shadowMode !== "off");
  }, [reads, sScope, heldKey, shadowMode]); // eslint-disable-line react-hooks/exhaustive-deps
  const liveData = view?.data ?? 0;
  useEffect(() => {
    if (shadowMode === "off" || liveData === 0) return;
    const t = setTimeout(() => reads.refresh(), SHADOW_SETTLE_MS);
    return () => clearTimeout(t);
  }, [liveData, shadowMode, reads]);
  const shadowNow = forScope(shadowState.data, sScope);
  const shadowRead = shadowState.scope === sScope ? shadowState : null;
  // This render's visibility (review B1), not the hidden attribute the canvas sets after.
  const shadowCtx = useMemo(() => (view ? { graph: view.graph, entities, shown: shownBy(view.graph, mask) } : null), [view, entities, mask]);
  const shadowPlaces = useMemo(
    () => (view && mergeEntities && layout.done ? mergedPlaces(view.graph, entities, shownBy(view.graph, mask)) : null),
    [view, mergeEntities, layout.done, entities, nudged, mask], // eslint-disable-line react-hooks/exhaustive-deps
  );
  const shadowAll = useMemo(
    () => (shadowNow && shadowCtx && shadowMode === "all" ? allShadows(shadowNow.index, shadowCtx, shadowPlaces) : null),
    [shadowNow, shadowCtx, shadowMode, shadowPlaces, liveData], // eslint-disable-line react-hooks/exhaustive-deps
  );
  const shadowSelected = useMemo(
    () => (shadowNow && shadowCtx && selected && view?.graph.hasNode(selected) ? nodeShadows(shadowNow.index, shadowCtx, selected, SELECTED_TOP, shadowPlaces) : []),
    [shadowNow, shadowCtx, selected, shadowPlaces, liveData], // eslint-disable-line react-hooks/exhaustive-deps
  );
  const shadowDrawn = useMemo(() => {
    if (!shadowNow || shadowMode === "off") return null;
    const hover = shadowMode === "selected" && shadowCtx ? (n: string) => nodeShadows(shadowNow.index, shadowCtx, n, HOVER_TOP, shadowPlaces) : undefined;
    const pairs = shadowMode === "all" ? (shadowAll?.pairs ?? []) : shadowSelected;
    return { pairs, hover };
  }, [shadowNow, shadowMode, shadowAll, shadowSelected, shadowCtx, shadowPlaces]);
  const shadowNotes = useMemo(() => {
    const notes: string[] = [];
    if (shadowRead?.error) notes.push(`Shadows could not be read: ${shadowRead.error}`);
    if (shadowRead && shadowRead.waiting.length > 0) notes.push(`Shadows of ${shadowRead.waiting.join(", ")} wait for Apply.`);
    const r = shadowNow;
    if (!r) return notes;
    if (r.tooLarge) notes.push("Too many to compare: close some graphs.");
    if (r.cut) notes.push(`Only the first ${number.format(SHADOW_ROWS_MAX)} pairs umx gave are read.`);
    for (const g of r.groups) if (g.model && g.floor === null) notes.push(`${g.model} is uncalibrated: its shadows are ranked only.`);
    if (r.groups.length > 1) notes.push(`Compared within each model: ${r.groups.map((g) => `${g.model ?? "unknown"} (${g.graphs.join(", ")})`).join("; ")}.`);
    const skipped = skippedLegend(r.skipped);
    if (skipped) notes.push(skipped);
    return notes;
  }, [shadowNow, shadowRead]);
  const shadowLegend: ShadowLegend | null = view
    ? {
        mode: shadowMode,
        onMode: setShadowMode,
        count: shadowDrawn?.pairs.length ?? 0,
        more: shadowMode === "all" ? (shadowAll?.more ?? 0) : 0,
        loading: shadowRead?.loading ?? false,
        notes: shadowNotes,
        onRefresh: () => reads.refresh(),
      }
    : null;
  const shadowPanel =
    shadowMode !== "off" && shadowNow && selected && view
      ? {
          rows: shadowSelected.map((p) => {
            const far = farEnd(p, selected, shadowPlaces);
            return { key: far, title: view.graph.hasNode(far) ? view.graph.getNodeAttribute(far, "label") : far, graph: aliasOfKey(far), score: p.score };
          }),
          notes: [
            ...skippedPanel(shadowNow.skipped),
            ...(shadowNow.groups.length > 1 ? [`Shadows are compared within each embedding model; none cross from one model's graphs to another's.`] : []),
          ],
        }
      : null;
  const colorOf = useCallback((alias: string) => graphColor(view?.ranks.get(alias) ?? 0, theme), [view, theme]);
  const entityView: EntityView | null =
    walked && view && selected && (walked.members.length > 1 || walked.proposed.length > 0 || walked.denied.length > 0 || (replyNow?.reply?.notOpen.length ?? 0) > 0)
      ? {
          walked,
          about: (key) => (view.graph.hasNode(key) ? { title: view.graph.getNodeAttribute(key, "label"), updated: view.graph.getNodeAttribute(key, "updated") ?? null } : { title: key, updated: null }),
          twice: entities.list[entities.of.get(selected) ?? -1]?.twice ?? [],
          reply: replyNow?.reply ?? null,
          loading: entityAsk !== null && (replyNow?.loading ?? true),
          error: replyNow?.error ?? null,
          merged: mergedRecord,
          onPick: pickNode,
          onAcross: () => showAcross(walked.members.map((m) => m.key), selected),
          onOpen: (g) => openGraphs([...shown, g]),
        }
      : null;

  // Each open graph's symmetric relations; keyed by their text, so a live change that keeps them redraws nothing.
  const symmetricKey = view ? JSON.stringify(view.snaps.map((sn) => [sn.graph, sn.symmetric ?? []])) : "";
  const symmetric = useMemo(() => symmetricByGraph(view?.snaps ?? []), [symmetricKey]); // eslint-disable-line react-hooks/exhaustive-deps
  // The selected node's drawn edges past the forced labels: the canvas counts them with the edge reducer's own
  // predicate (reviewer R370-2), so a denial or a threaded identity edge is never counted.
  const [unlabelled, setUnlabelled] = useState(0);

  const readouts = useMemo<ReadoutValues | null>(
    () =>
      view && stats
        ? {
            nodes: [stats.shownNodes, total(view.snaps, "totalNodes")],
            links: [stats.shownEdges, total(view.snaps, "totalEdges")],
            groups: groups ? { count: groups.sizes.filter((n) => n >= HULL_MIN).length, method: grouping } : null,
            hidden: stats.toggledOff,
            focus: shownFocus ? { hops: shownFocus.hops, nodes: shownFocus.depths.size } : null,
            selected: selection.ids.length,
            agent: agent.state.history[0] ? { seq: agent.state.history[0].seq, at_ms: agent.state.history[0].at_ms } : null,
            layout: { done: layout.done, iterations: layout.iterations, ms: layout.ms, capped: layout.capped },
            pending: liveModeNow === "hold" ? heldAll : undefined,
          }
        : null,
    [view, stats, groups, grouping, shownFocus, selection.ids.length, agent.state.history, layout, liveModeNow, heldAll],
  );

  const stripHistory = useMemo<StripEntry[]>(
    () =>
      agent.state.history
        .filter((e) => !openIgnored(e, securedAliases))
        .map((e) => ({ seq: e.seq, at_ms: e.at_ms, d: describeEvent(e, titleOf), replay: cameraAsk(e) !== null || e.command.kind === "highlight" })),
    [agent.state.history, titleOf, securedAliases],
  );
  const replay = useCallback(
    (seq: number) => {
      const e = agent.state.history.find((h) => h.seq === seq);
      if (!e) return;
      const ask = cameraAsk(e) ?? (e.command.kind === "highlight" ? { seq, at_ms: e.at_ms, kind: "fit" as const, nodes: e.command.nodes, graph: e.graph } : null);
      if (ask && (ask.nodes.length === 0 || ask.nodes.some(shownNode))) moveCamera(ask);
    },
    [agent.state.history, shownNode, moveCamera],
  );
  const undoAgent = useCallback(() => updateLedger(agentOff), [updateLedger]);
  const promptTarget = prompt ? describeEvent({ seq: prompt.seq, at_ms: prompt.at_ms, graph: prompt.graph, by: null, command: prompt.kind === "focus" ? { kind: "focus", node: prompt.nodes[0], hops: 1 } : { kind: "fit", nodes: prompt.nodes } }, titleOf).target.replace(/, 1 hop$/, "") : null;


  const wantedList = wanted ? wanted.split(",") : [];
  const reading = wantedList.length === 1 ? graphs?.find((g) => g.alias === wantedList[0]) : undefined;
  // Live when every open graph's revision is known (from its snapshot, then from each change applied).
  const liveRevs = view ? view.snaps.map((sn) => liveAt.get(sn.graph) ?? sn.live ?? null) : [];
  const liveError = view ? (view.aliases.map((a) => liveErrors.get(a)).find((e) => e !== undefined) ?? null) : null;
  const liveOn = view !== null && liveAll(liveRevs, liveErrors, view.aliases) && agent.error === null;
  const liveRev = liveRevs.reduce((n, r) => n + (r?.rev ?? 0), 0);
  const totalNodes = view ? total(view.snaps, "totalNodes") : 0;
  const totalEdges = view ? total(view.snaps, "totalEdges") : 0;
  const truncated = view?.snaps.some((sn) => sn.truncated) ?? false;
  const leftNote = leftOutNote(set.leftOut);
  // umx's hints from the reads that opened the graphs (D69 (9)): warnings that name a next command, and any hints.
  const hints = view
    ? hintItems(
        view.snaps.flatMap((sn) => (sn.warnings ?? []).filter((w) => w.code !== "graph_loaded_on_demand").map((w) => (multi ? { ...w, message: `${sn.graph}: ${w.message}` } : w))),
        view.snaps.flatMap((sn) => (sn.hints ?? []).filter((h) => h.code !== "graph_loaded_on_demand").map((h) => (multi ? { ...h, message: `${sn.graph}: ${h.message}` } : h))),
      )
    : [];
  const outOfPlay = !visible.retired ? (stats?.retired ?? 0) : 0;
  const liveTitle = liveOn
    ? "Changes made through umx show here as they happen"
    : liveError
      ? `A change could not be shown yet (${liveError.message}); trying again`
      : "Waiting for the umx daemon's change feed";
  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full flex-col">
        <header className="fui-frame relative z-30 flex h-12 shrink-0 items-center gap-3 border-b bg-card/80 px-4 backdrop-blur">
          <Waypoints className="size-5 text-primary" aria-hidden />
          <span className="text-sm font-semibold tracking-tight fui:font-mono fui:uppercase">umx</span>
          <span className="text-muted-foreground">/</span>
          <span className="max-w-[24rem] shrink-0 truncate text-sm font-medium fui:font-mono" data-testid="graph-name" title={shown.join(", ")}>
            {/* More than 3 read as a count; the full list is the tooltip and the graph list (ui-expert N2). */}
            {graphNames(shown.length > 0 ? shown : wantedList)}
          </span>
          {view && stats ? (
            <span className="min-w-0 truncate text-xs whitespace-nowrap text-muted-foreground tabular-nums fui:font-mono fui:text-[11px] fui:tracking-[0.08em] fui:uppercase" data-testid="counts">
              {/* Held, the counts are of the revisions last applied (ui-expert N3). */}
              {liveModeNow === "hold" ? <span data-testid="last-applied">Last applied: </span> : null}
              {number.format(stats.shownNodes)} of {number.format(totalNodes)} nodes ·{" "}
              {number.format(stats.shownEdges)} of {number.format(totalEdges)} edges
              {multi ? ` · ${number.format(stats.external.drawn)} between graphs` : ""}
              {omittedNote(stats.external, view.dropped)}
              {leftNote ? <span data-testid="left-out"> · {leftNote}</span> : null}
            </span>
          ) : null}
          {view ? (
            <span className="flex shrink-0 items-center gap-1.5 text-xs whitespace-nowrap text-muted-foreground" data-testid="live" data-live={liveOn ? "on" : "off"} data-mode={liveModeNow} title={liveModeNow === "hold" ? "Changes made through umx are held until you apply them" : liveTitle}>
              {/* Live or Hold (plan 152): one switch for every open graph. */}
              <button
                type="button"
                role="switch"
                aria-checked={liveModeNow === "hold"}
                aria-label="Hold changes"
                onClick={() => switchMode(liveModeNow === "hold" ? "live" : "hold")}
                className="flex items-center gap-1.5 rounded-md px-1.5 py-0.5 hover:bg-muted fui:font-mono fui:uppercase"
                data-testid="live-mode"
              >
                {liveModeNow === "hold" ? (
                  <>
                    <span className="size-2 rounded-full bg-amber-500 fui:bg-primary" aria-hidden />
                    <span data-testid="held" data-count={heldAll ?? "many"}>
                      Held{heldAll !== 0 ? ` · ${countText(heldAll)} ${heldAll === 1 ? "change" : "changes"}` : ""}
                    </span>
                  </>
                ) : (
                  <>
                    <Radio className={cn("size-3.5", liveOn ? "text-emerald-600 dark:text-emerald-400" : "opacity-50")} aria-hidden />
                    {liveOn ? "Live" : liveError ? "Live: retrying" : "Not live"}
                  </>
                )}
              </button>
              {liveModeNow === "hold" && heldAll !== 0 ? (
                <button type="button" onClick={() => applyHeld(view.aliases)} className="rounded-md border px-1.5 py-0.5 text-xs hover:bg-muted fui:font-mono fui:uppercase" data-testid="apply-all">
                  Apply all
                </button>
              ) : null}
              {liveNote ? (
                <span className="tabular-nums" data-testid="live-note" data-n={liveNote.n}>
                  · {liveNote.text}
                </span>
              ) : null}
            </span>
          ) : null}
          <div className="ml-auto flex shrink-0 items-center gap-2 whitespace-nowrap">
            {view && !layout.done ? (
              <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
                <Loader2 className="size-3.5 animate-spin" /> Laying out {Math.round(layout.progress * 100)}%
              </span>
            ) : null}
            {view ? <SelectionShare count={selection.ids.length} sharing={share.sharing} failures={share.failures} onSharing={share.setSharing} /> : null}
            {view && entities.list.length > 0 ? (
              <label className="ml-2 flex items-center gap-1.5 text-xs text-muted-foreground" title="Draw each entity as one node; nothing is stored">
                <Switch checked={mergeEntities} onCheckedChange={setMergeEntities} aria-label="Merge entities" data-testid="merge-entities" />
                Merge entities
              </label>
            ) : null}
            <span className="ml-2 text-xs text-muted-foreground">Colour by</span>
            <Segmented label="Colour by" testId="colour-by" value={colourBy} options={COLOUR_BY} onChange={setColourBy} />
            <label className="ml-2 flex items-center gap-1.5 text-xs text-muted-foreground" title="Agents may move the camera; never while you are using the canvas">
              <Switch checked={follow} onCheckedChange={setFollow} aria-label="Follow agent" data-testid="follow" />
              Follow agent
            </label>
            <ThemeToggle />
          </div>
        </header>
        <div className="flex min-h-0 flex-1">
          <Sidebar
            picker={{
              graphs,
              scopes,
              open: shown.length > 0 ? shown : wantedList,
              held: heldCounts.size > 0 ? { counts: heldCounts, onApply: (a) => applyHeld([a]) } : undefined,
              loading: opening,
              onOpen: openGraphs,
              includeSecured: set.includeSecured,
              onIncludeSecured: set.setIncludeSecured,
              choosing,
              onChoosing: setChoosing,
              theme,
            }}
            spine={{
              rows: shown.map((a) => {
                const name = spineShown.get(a) ?? null;
                const r = name ? spinePlaces.get(`${a}/${name}`) : undefined;
                return { alias: a, spines: spinesOf(a), shown: name, state: spineState(r) };
              }),
              multi,
              onPick: (a, name) => setSpinePicks((m) => new Map(m).set(a, name)),
              spread,
              onSpread: setSpread,
            }}
            graphCounts={stats?.graphs ?? new Map()}
            graph={view?.graph ?? null}
            version={version}
            theme={theme}
            colourBy={colourBy}
            groups={groups}
            groupLabels={groupInfo?.labels ?? []}
            statuses={stats?.statuses ?? new Map()}
            grouping={grouping}
            onGrouping={setGrouping}
            resolution={resolution}
            onResolution={setResolution}
            spacing={spacing}
            layout={layoutOptions}
            onLayout={setLayoutOptions}
            hubCount={hubs.centres.length}
            hubFallback={hubs.fallback ?? false}
            onSpacing={setSpacing}
            louvainMs={louvain?.ms ?? null}
            clustering={view !== null && grouping === "louvain" && (louvain === null || louvain.resolution !== resolution)}
            query={query}
            onQuery={setQuery}
            mode={mode}
            onMode={setMode}
            search={search}
            place={place}
            onPickNode={pickHit}
            onReveal={revealHit}
            filters={filters}
            pulse={pulse}
            chips={chips}
            onChip={onChip}
            onResetFilters={() => {
              updateLedger((l) => personSets(l, NO_FILTERS, "all"));
              setRelationModes(new Map());
            }}
            shadows={shadowLegend}
            relationLook={(relation) => ({ mode: relationMode(relation, relationModes, filters.relations), symmetric: relationSymmetric(relation, symmetric) })}
            modesChanged={relationModes.size > 0}
            visible={visible}
            onVisible={setVisible}
            hiddenCounts={{ provenance: stats?.provenance ?? 0, retired: stats?.retired ?? 0 }}
            types={stats?.types ?? []}
            searchRef={searchRef}
          />
          <main
            className="relative min-w-0 flex-1 overflow-hidden bg-canvas"
            data-layout={layout.error ? "error" : layout.capped ? "incomplete" : layout.done ? "done" : "running"}
            data-layout-mode={layoutOptions.mode}
            data-hubs={hubs.centres.length}
            data-hub-ms={hubs.ms}
            data-layout-ms={layout.ms}
            data-layout-iterations={layout.iterations}
            data-layout-capped={layout.capped ? "yes" : layout.done ? "no" : undefined}
            data-louvain-ms={louvain?.ms}
            data-groups={groups ? groups.sizes.filter((n) => n >= HULL_MIN).length : undefined}
            data-grouping={grouping}
            data-focus={focus.state}
            data-focus-count={focus.state === "on" ? focus.count : undefined}
            data-fetch-ms={view?.timing.fetchMs}
            data-shown-nodes={stats?.shownNodes}
            data-shown-edges={stats?.shownEdges}
            data-selected={selected ?? undefined}
            data-selection={selection.ids.length}
            data-jumped={jumped ?? undefined}
            data-agent-highlight={agent.state.highlight.size}
            data-agent-focus={agentFocus ? agentFocus.size : undefined}
            data-agent-seq={agent.state.history[0]?.seq}
            data-build-ms={view?.timing.buildMs}
            data-live-rev={view ? liveRev : undefined}
            data-live-changes={view?.data}
            data-total-nodes={view ? totalNodes : undefined}
            data-total-edges={view ? totalEdges : undefined}
            data-graphs={view ? shown.length : undefined}
            data-entities={view ? entities.list.length : undefined}
            data-proposed-pairs={view ? entities.proposed.length : undefined}
            data-merge={mergeEntities ? "on" : "off"}
            data-across={acrossNow ? acrossNow.members.length : 0}
            data-across-focus={acrossDepths?.size ?? 0}
            data-shadow-mode={shadowMode}
            data-shadow-pairs={shadowDrawn?.pairs.length ?? 0}
            data-shadow-graphs={shadowDrawn ? [...new Set(shadowDrawn.pairs.flatMap((p) => [aliasOfKey(p.a), aliasOfKey(p.b)]))].sort().join(",") : ""}
            data-shadow-read={shadowRead ? (shadowRead.loading ? "loading" : shadowRead.error ? "error" : shadowRead.data ? "done" : "none") : "none"}
            data-shadow-ms={shadowRead?.ms ?? undefined}
            data-graph-overlap={overlap?.toFixed(3)}
            data-external-drawn={stats?.external.drawn}
            data-external-omitted={stats ? stats.external.notOpen + stats.external.broken : undefined}
          >
            {layout.error || layout.capped ? <div className="absolute bottom-3 left-3 z-10 rounded-md border bg-card px-3 py-2 text-sm" role="status">
              {layout.error ? `Layout failed: ${layout.error}` : "Layout incomplete: time limit reached. Change the layout or filters to retry."}
            </div> : null}
            {truncated && view ? (
              <div className="absolute top-3 left-1/2 z-10 flex -translate-x-1/2 items-center gap-2 rounded-lg border border-amber-500/40 bg-amber-50 px-3 py-1.5 text-sm text-amber-900 shadow-sm dark:bg-amber-950 dark:text-amber-100" data-testid="truncated">
                <AlertTriangle className="size-4" />
                Truncated: showing {number.format(view.graph.order)} of {number.format(totalNodes)} nodes and{" "}
                {number.format(view.graph.size)} of {number.format(totalEdges)} edges.
              </div>
            ) : null}
            {view && (outOfPlay > 0 || (hints.length > 0 && hintsHidden !== view.key)) ? (
              <div className="absolute top-3 left-3 z-10 max-w-[min(28rem,45%)] space-y-1.5">
                {outOfPlay > 0 ? (
                  <div className="flex items-center gap-2 rounded-md border bg-card/95 px-2 py-1 text-xs shadow-sm" data-testid="out-of-play-hint" data-count={outOfPlay}>
                    <span className="tabular-nums">
                      {number.format(outOfPlay)} out-of-play node{outOfPlay === 1 ? "" : "s"} hidden
                    </span>
                    <button type="button" className="text-primary hover:underline" onClick={() => setVisible((v) => ({ ...v, retired: true }))}>
                      show
                    </button>
                  </div>
                ) : null}
                {hints.length > 0 && hintsHidden !== view.key ? (
                  <div className="space-y-1" data-testid="view-hints">
                    <Hints items={hints} testId="snapshot-hints" max={3} />
                    <button type="button" className="text-[11px] text-muted-foreground hover:text-foreground" onClick={() => setHintsHidden(view.key)}>
                      Hide hints
                    </button>
                  </div>
                ) : null}
              </div>
            ) : null}
            {notice ? (
              <div
                className="absolute top-3 left-1/2 z-20 flex max-w-[min(36rem,90%)] -translate-x-1/2 items-start gap-2 rounded-lg border bg-card px-3 py-2 text-sm shadow-sm"
                data-testid="notice"
                data-code={notice.issue.code}
              >
                <Info className="mt-0.5 size-4 shrink-0 text-primary" />
                <div className="min-w-0 space-y-0.5">
                  <p className="break-words">{notice.issue.message}</p>
                  {notice.issue.next ? <p className="font-mono text-xs break-all text-muted-foreground select-all">{notice.issue.next}</p> : null}
                </div>
                {notice.sticky ? (
                  <button type="button" className="ml-1 text-muted-foreground hover:text-foreground" aria-label="Dismiss" onClick={() => setNotice(null)}>
                    <X className="size-4" />
                  </button>
                ) : null}
              </div>
            ) : null}
            {ended ? (
              <div className="absolute inset-0 z-40 grid place-items-center bg-background/70 p-8 backdrop-blur-sm" data-testid="view-ended">
                <div className="max-w-md space-y-1 rounded-xl border bg-card p-5 shadow-sm">
                  <p className="font-mono text-xs text-muted-foreground">{ended.code}</p>
                  <p className="text-sm">{ended.message}</p>
                </div>
              </div>
            ) : null}
            {error ? (
              <div className="absolute inset-0 z-10 grid place-items-center p-8">
                <div className="max-w-md space-y-1 rounded-xl border bg-card p-5 shadow-sm">
                  <p className="font-mono text-xs text-destructive">{error.code}</p>
                  <p className="text-sm">{error.message}</p>
                  {error.next ? <p className="font-mono text-xs text-muted-foreground">next: {error.next}</p> : null}
                </div>
              </div>
            ) : null}
            {!view && !loading && !error && !wanted && graphs && graphs.length > 0 ? (
              <div className="absolute inset-0 z-10 grid place-items-center p-8" data-testid="no-graphs-open">
                <div className="max-w-sm space-y-3 text-center">
                  <p className="text-sm font-medium">No graphs open</p>
                  <p className="text-sm text-muted-foreground">Choose one or more of your {number.format(graphs.length)} graphs to lay out here.</p>
                  <Button size="sm" onClick={() => setChoosing(true)}>
                    Choose graphs <kbd className="rounded bg-primary-foreground/15 px-1 font-mono text-[10px]">G</kbd>
                  </Button>
                </div>
              </div>
            ) : null}
            {loading ? (
              <div className="absolute inset-0 z-10 grid place-items-center">
                <span className="flex items-center gap-2 text-sm text-muted-foreground">
                  <Loader2 className="size-4 animate-spin" /> Reading {wantedList.join(", ")}
                  {reading?.nodes != null ? ` (${number.format(reading.nodes)} nodes)` : ""}…
                </span>
              </div>
            ) : null}
            {view && palette ? (
              <GraphCanvas
                key={view.key}
                graph={view.graph}
                hold={view.frame ?? null}
                spineAxes={spineAxes}
                camera={view.camera ?? null}
                cameraOut={cameraOut}
                palette={palette}
                selected={selected}
                selection={selectionSet}
                jump={jump}
                focused={acrossDepths ?? shownFocus?.depths ?? null}
                groups={spineAxes.length > 0 ? null : groupShown}
                layoutDone={layout.done}
                spacing={spacing}
                layoutMode={layoutOptions.mode}
                spineKey={[...spineShown].map(([alias, name]) => `${alias}/${name ?? ""}`).join(",")}
                onUnlabelled={setUnlabelled}
                offRelations={filters.relations}
                relationModes={relationModes}
                symmetric={symmetric}
                version={version}
                onPick={onPick}
                onClear={onClear}
                onLasso={onLasso}
                onJumped={setJumped}
                agentMarks={agent.state.highlight}
                entities={entities}
                merge={mergeEntities}
                across={acrossSet}
                colorOf={colorOf}
                shadows={shadowDrawn}
                fit={fit}
                gesture={gesture}
                agentFocusNode={agent.state.focus?.node ?? null}
                agentNotes={agentNotes}
                live={liveTouched}
                readouts={readouts}
                panelOpen={selected !== null || removedId !== null}
              />
            ) : null}
            {view ? (
              <AgentStrip
                history={stripHistory}
                onUndo={
                  undoOpen && view.key === undoOpen.after && stripHistory[0]?.seq === undoOpen.seq
                    ? undoAgentOpen
                    : ledger.agent.length > 0 && ledger.seq === agent.state.filter?.seq
                      ? undoAgent
                      : null
                }
                prompt={promptTarget !== null ? { target: promptTarget } : null}
                onGo={() => {
                  if (prompt) moveCamera(prompt);
                  setPrompt(null);
                }}
                onDismiss={() => setPrompt(null)}
                onReplay={replay}
                error={agent.error && agent.error.code !== "ui_events_pending" ? agent.error : null}
                notice={shownNote ? { text: shownNote.text, action: shownNote.apply ? { label: "Apply", run: () => applyHeld([shownNote.apply!]) } : undefined } : null}
                onNoticeDismiss={() => setHoldNote(null)}
                openBox={openBox ?? offerBox}
                onOpenDismiss={() => (openBox ? setOpenBox(null) : setOffer(null))}
              />
            ) : null}
            <NodePanel
              open={selected !== null || removedId !== null}
              removed={selected === null && removedId !== null}
              detail={detail}
              error={detailError}
              loading={detailLoading}
              theme={theme}
              onClose={onClosePanel}
              onPick={pickNode}
              hiddenIds={hiddenIds}
              graph={selected ? aliasOfKey(selected) : null}
              links={links}
              focus={{ depth: focusDepth, focus, onDepth: setFocusDepth }}
              unlabelled={unlabelled}
              entity={entityView}
              shadows={shadowPanel}
              spines={nodeSpines}
              similar={{
                loading: similarNow?.loading ?? false,
                result: similarNow?.result ?? null,
                error: similarNow?.error ?? null,
                onAsk: askSimilar,
                place,
                onReveal: revealHit,
              }}
            />
          </main>
        </div>
      </div>
    </TooltipProvider>
  );
}
