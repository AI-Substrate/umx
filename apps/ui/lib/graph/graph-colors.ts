// Colour by graph (ws009 M1a, plan 133): one hue per open graph, in the order the graphs are open. Its own palette
// function, apart from type, cluster and status colours (lib/graph/colors), so the skins can restyle those freely.
import { oklchToHex, type Theme } from "@/lib/graph/colors";

/** Hues far apart from each other, none of them risk red (25): a graph's colour never reads as a warning. */
const HUES = [250, 145, 318, 55, 190, 275, 95];
/** Per theme, the K1 skins included: Console sits on a dark ground, Instrument on a light one. */
const TONE = {
  light: { l: 0.6, c: 0.16 },
  dark: { l: 0.76, c: 0.14 },
  console: { l: 0.8, c: 0.13 },
  instrument: { l: 0.56, c: 0.16 },
};
const LIGHT_GROUND = new Set(["light", "instrument"]);

/** The fill for nodes of the graph open at `rank` (0 is the first). Past seven graphs, hues repeat a tier off. */
export function graphColor(rank: number, theme: Theme, muted = false): string {
  const tone = TONE[theme as keyof typeof TONE] ?? TONE.light;
  const r = Math.max(0, rank);
  const tier = Math.floor(r / HUES.length);
  const l = tone.l + (tier % 2 === 1 ? (LIGHT_GROUND.has(theme) ? 0.12 : -0.12) : 0);
  return oklchToHex(l, tone.c * (muted ? 0.4 : 1), HUES[r % HUES.length]);
}
