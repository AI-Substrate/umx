// Spacing changes force distances and collision clearance in the worker, never nominal glyph sizes.

// The slider is logarithmic: each of its `steps` multiplies the spacing by the same factor, so the low end keeps fine
// control and the top end reaches 10 times the old maximum of 3. Stored values from the old 0.5 to 3 slider still
// read, and lay out exactly as they did.
export const SPACING = { min: 0.5, max: 30, steps: 100, default: 1.6 } as const;
/** Up to this spacing the force layout grows slower than the spacing; above it, in step with it (lib/graph/layout
 * FORCE), so whatever scales by the layout's size needs no extra spacing factor there. */
export const SPACING_LINEAR = 3;
/** Where the view keeps the person's spacing, a number only, as next-themes keeps the theme. */
export const SPACING_KEY = "layout-spacing";

/** A stored spacing, or the default for anything that is not a number in range. */
export function spacingOf(stored: string | null): number {
  const v = stored === null || stored.trim() === "" ? NaN : Number(stored);
  return Number.isFinite(v) && v >= SPACING.min && v <= SPACING.max ? v : SPACING.default;
}

/** The spacing at a slider position (0 to SPACING.steps), to 3 significant figures so it reads and stores cleanly
 * and maps back to the same position. */
export function spacingAt(position: number): number {
  const t = Math.min(1, Math.max(0, position / SPACING.steps));
  return Math.min(SPACING.max, Math.max(SPACING.min, Number((SPACING.min * (SPACING.max / SPACING.min) ** t).toPrecision(3))));
}

/** The slider position of a spacing: the inverse of spacingAt, fractional for values between its steps. */
export function positionOf(spacing: number): number {
  const s = Math.min(SPACING.max, Math.max(SPACING.min, spacing));
  return (SPACING.steps * Math.log(s / SPACING.min)) / Math.log(SPACING.max / SPACING.min);
}

/** The slider's readout: one decimal below 10, whole numbers above. */
export function spacingLabel(spacing: number): string {
  return spacing < 10 ? spacing.toFixed(1) : spacing.toFixed(0);
}
