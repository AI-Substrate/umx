"use client";
// The open set of graphs as state (ws009 M1a, plan 133; D69 (7)): what is named, what a scope or "all" left out, and
// the include-secured toggle. The toggle starts off on every load and lives only in this state: never in storage or
// the URL, and nothing an agent sends reaches it. A picker changes the set only through `open`; a launch link only
// through `launch`.
import { useCallback, useMemo, useRef, useState } from "react";
import type { LeftOut } from "@/lib/graph-set";
import { setKey } from "@/lib/graph/keys";

export type GraphSet = {
  /** The named graphs as their set key ("" for none yet), and as a list in open order. */
  key: string;
  aliases: string[];
  leftOut: LeftOut | null;
  includeSecured: boolean;
  setIncludeSecured: (on: boolean) => void;
  /** Names exactly these graphs (a pick, "Open all" or a scope); `left` is what it left out, counted. None clears the
   * canvas, and a relist (a launch that is not fresh) leaves it clear. */
  open: (aliases: string[], left?: LeftOut) => void;
  /** A launch link's graphs: `fresh` (a new link in an open tab) switches to them; the first one only fills an empty view. */
  launch: (aliases: string[], fresh: boolean) => void;
  /** What opened once reads failed (ws009 M1b2, review B2): the set becomes exactly these, none included, so
   * picking a graph whose read failed reads it again. What a scope left out stays said. */
  settle: (aliases: string[]) => void;
};

export function useGraphSet(): GraphSet {
  const [key, setKeyState] = useState("");
  const [leftOut, setLeftOut] = useState<LeftOut | null>(null);
  const [includeSecured, setIncludeSecured] = useState(false);
  // The person emptied the canvas: a launch that is not fresh does not fill it again.
  const cleared = useRef(false);
  const open = useCallback((aliases: string[], left?: LeftOut) => {
    cleared.current = aliases.length === 0;
    setLeftOut(left && left.secured + left.unavailable > 0 ? left : null);
    setKeyState(setKey(aliases));
  }, []);
  const launch = useCallback((aliases: string[], fresh: boolean) => {
    if (aliases.length === 0) return;
    if (fresh) {
      cleared.current = false;
      setLeftOut(null);
      setKeyState(setKey(aliases));
    } else if (!cleared.current) setKeyState((k) => k || setKey(aliases));
  }, []);
  const settle = useCallback((aliases: string[]) => setKeyState(setKey(aliases)), []);
  const aliases = useMemo(() => (key ? key.split(",") : []), [key]);
  return { key, aliases, leftOut, includeSecured, setIncludeSecured, open, launch, settle };
}
