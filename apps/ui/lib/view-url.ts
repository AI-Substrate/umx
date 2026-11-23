// The view's look kept across a reload: in the fragment (how the canvas looks), and the open graphs in the tab's
// sessionStorage. A fragment any page can write may only change how the view looks, never what it reads (security
// N1, lib/launch): so no graph is ever read from it. The open set lives in sessionStorage, which only this origin and
// this tab can write; the include-secured switch is never kept (D69 (7)).
import type { ColourBy } from "@/lib/graph/build";
import { MAX_OPEN } from "@/lib/graph/keys";
import { SPACING } from "@/lib/graph/spacing";

export type Grouping = "louvain" | "parent" | "graph";

/** What the fragment keeps. A field left out is the view's default. */
export type ViewLook = {
  colourBy?: ColourBy;
  grouping?: Grouping;
  resolution?: number;
  spacing?: number;
  merge?: boolean;
  provenance?: boolean;
  retired?: boolean;
  off?: { relations: string[]; types: string[]; statuses: string[] };
};

export const LOOK_DEFAULTS = { colourBy: "type", resolution: 1, merge: true, provenance: false, retired: false } as const;

const COLOURS: readonly ColourBy[] = ["type", "cluster", "status", "graph"];
const GROUPINGS: readonly Grouping[] = ["louvain", "parent", "graph"];
const RESOLUTION = { min: 0.1, max: 3 };
/** Names in the fragment: a chip's key is a type, relation or status name; anything longer or odder is dropped. */
const NAME = /^[A-Za-z0-9_:.-]{1,64}$/;
const OFF_MAX = 200;
const KEYS = { colourBy: "colour", grouping: "group", resolution: "res", spacing: "spacing", merge: "merge", provenance: "prov", retired: "retired" } as const;
const OFF = { relations: "off.rel", types: "off.type", statuses: "off.status" } as const;
/** Launch-link keys (lib/launch) this module never writes and leaves to takeLaunch. */
const LAUNCH = new Set(["t", "g"]);

function params(hash: string): URLSearchParams {
  return new URLSearchParams(hash.startsWith("#") ? hash.slice(1) : hash);
}

function num(v: string | null, min: number, max: number): number | undefined {
  if (v === null || v.trim() === "") return undefined;
  const n = Number(v);
  return Number.isFinite(n) && n >= min && n <= max ? n : undefined;
}

function flag(v: string | null): boolean | undefined {
  return v === "1" ? true : v === "0" ? false : undefined;
}

function names(v: string | null): string[] {
  if (!v) return [];
  return [...new Set(v.split(",").filter((s) => NAME.test(s)))].slice(0, OFF_MAX);
}

/** The look a fragment holds: each value checked, anything unknown or out of range left out. */
export function readLook(hash: string): ViewLook {
  const p = params(hash);
  const look: ViewLook = {};
  const colour = p.get(KEYS.colourBy);
  if (colour && (COLOURS as readonly string[]).includes(colour)) look.colourBy = colour as ColourBy;
  const group = p.get(KEYS.grouping);
  if (group && (GROUPINGS as readonly string[]).includes(group)) look.grouping = group as Grouping;
  const res = num(p.get(KEYS.resolution), RESOLUTION.min, RESOLUTION.max);
  if (res !== undefined) look.resolution = res;
  const spacing = num(p.get(KEYS.spacing), SPACING.min, SPACING.max);
  if (spacing !== undefined) look.spacing = spacing;
  for (const k of ["merge", "provenance", "retired"] as const) {
    const f = flag(p.get(KEYS[k]));
    if (f !== undefined) look[k] = f;
  }
  const off = { relations: names(p.get(OFF.relations)), types: names(p.get(OFF.types)), statuses: names(p.get(OFF.statuses)) };
  if (off.relations.length + off.types.length + off.statuses.length > 0) look.off = off;
  return look;
}

/** The fragment for `look`: only what differs from the defaults, in a fixed order; "" when nothing does. */
export function writeLook(look: ViewLook): string {
  const p = new URLSearchParams();
  if (look.colourBy && look.colourBy !== LOOK_DEFAULTS.colourBy) p.set(KEYS.colourBy, look.colourBy);
  if (look.grouping) p.set(KEYS.grouping, look.grouping);
  if (look.resolution !== undefined && look.resolution !== LOOK_DEFAULTS.resolution) p.set(KEYS.resolution, String(look.resolution));
  if (look.spacing !== undefined && look.spacing !== SPACING.default) p.set(KEYS.spacing, String(look.spacing));
  for (const k of ["merge", "provenance", "retired"] as const) {
    if (look[k] !== undefined && look[k] !== LOOK_DEFAULTS[k]) p.set(KEYS[k], look[k] ? "1" : "0");
  }
  for (const k of ["relations", "types", "statuses"] as const) {
    const list = (look.off?.[k] ?? []).filter((s) => NAME.test(s)).slice(0, OFF_MAX);
    if (list.length > 0) p.set(OFF[k], [...list].sort().join(","));
  }
  // Commas read better unescaped; URLSearchParams decodes them either way.
  return p.toString().replaceAll("%2C", ",");
}

/** Whether a fragment holds a launch link's keys (a token or graphs): takeLaunch reads and drops those. */
export function isLaunch(hash: string): boolean {
  return [...params(hash).keys()].some((k) => LAUNCH.has(k));
}

/** The tab's sessionStorage key for the open graphs. */
export const OPEN_KEY = "umx-ui.open";

/** The open graphs a tab kept: aliases as umx writes them, each once, at most MAX_OPEN; [] for none kept on purpose,
 * null for nothing kept (or anything unreadable). */
export function readOpen(stored: string | null): string[] | null {
  if (stored === null) return null;
  try {
    const v: unknown = JSON.parse(stored);
    if (!Array.isArray(v) || !v.every((a) => typeof a === "string" && NAME.test(a))) return null;
    return [...new Set(v as string[])].slice(0, MAX_OPEN);
  } catch {
    return null;
  }
}

/** What a page load opens: a launch link's graphs, else the ones this tab kept (none kept on purpose leaves the canvas
 * clear: the caller clears the set), else null for the first graph, as before. */
export function reopenGraphs(named: readonly string[] | null, kept: readonly string[] | null): string[] | null {
  if (named) return [...named];
  return kept && kept.length > 0 ? [...kept] : null;
}
