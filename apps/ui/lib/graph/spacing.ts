// Spacing changes force distances and collision clearance in the worker, never nominal glyph sizes.

export const SPACING = { min: 0.5, max: 3, step: 0.1, default: 1.6 } as const;
/** Where the view keeps the person's spacing, a number only, as next-themes keeps the theme. */
export const SPACING_KEY = "layout-spacing";

/** A stored spacing, or the default for anything that is not a number in range. */
export function spacingOf(stored: string | null): number {
  const v = stored === null || stored.trim() === "" ? NaN : Number(stored);
  return Number.isFinite(v) && v >= SPACING.min && v <= SPACING.max ? v : SPACING.default;
}

