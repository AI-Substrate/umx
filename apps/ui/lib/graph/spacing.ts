// The Spacing slider (plan 198; the ui-expert's spacing rulings r1 and r2): it multiplies ForceAtlas2's repulsion and
// noverlap's margin (lib/graph/layout, in the worker), which set the layout's scale, and the drawn node size, which is
// what changes the picture at Fit. Apart from the layout so the page does not load it.

export const SPACING = { min: 0.5, max: 3, step: 0.1, default: 1.6 } as const;
/** Where the view keeps the person's spacing, a number only, as next-themes keeps the theme. */
export const SPACING_KEY = "layout-spacing";

/** A stored spacing, or the default for anything that is not a number in range. */
export function spacingOf(stored: string | null): number {
  const v = stored === null || stored.trim() === "" ? NaN : Number(stored);
  return Number.isFinite(v) && v >= SPACING.min && v <= SPACING.max ? v : SPACING.default;
}

/** The drawn node size at a spacing (ruling r2's fallback, sized by the ui-expert in review): sqrt(default / spacing),
 * so nodes are today's size at the default and the gap between their edges at Fit grows with the spacing. */
export function nodeScale(spacing: number): number {
  return Math.sqrt(SPACING.default / spacing);
}
