export type Disc = { x: number; y: number; radius: number };

/** Spatially packs discs near their preferred positions. The shared fallback cursor bounds work even when every
 * preferred position is identical. Callers supply stable order; no random source or wall clock decides a position. */
export function separateDiscs(discs: readonly Disc[], fixed?: Uint8Array, bounds?: { left: number; right: number; top?: number }): Disc[] {
  if (discs.length === 0) return [];
  const radii = discs.map((d) => d.radius).sort((a, b) => a - b);
  const cell = Math.max(bounds ? 1e-6 : 1, radii[radii.length >> 1] * 2);
  const pitch = radii[radii.length - 1] * 2.01;
  const columns = bounds ? Math.max(1, Math.floor((bounds.right - bounds.left) / pitch)) : 1;
  const middle = bounds ? discs.reduce((sum, d) => sum + d.y / discs.length, 0) : 0;
  const constrain = (d: Disc): Disc => bounds ? {
    ...d, x: Math.max(bounds.left + d.radius, Math.min(bounds.right - d.radius, d.x)),
    y: bounds.top === undefined ? d.y : Math.min(bounds.top - d.radius, d.y),
  } : d;
  const buckets = new Map<number, Map<number, number[]>>();
  const out: Disc[] = new Array(discs.length);
  let fallback = 0;
  const visit = (d: Disc, read: (x: number, y: number) => boolean) => {
    const [x0, x1, y0, y1] = [Math.floor((d.x - d.radius) / cell), Math.floor((d.x + d.radius) / cell), Math.floor((d.y - d.radius) / cell), Math.floor((d.y + d.radius) / cell)];
    for (let x = x0; x <= x1; x++) {
      for (let y = y0; y <= y1; y++) {
        if (!read(x, y)) return false;
      }
    }
    return true;
  };
  const clear = (d: Disc) => visit(d, (x, y) => {
    for (const i of buckets.get(x)?.get(y) ?? []) {
      const b = out[i];
      if (Math.hypot(d.x - b.x, d.y - b.y) < d.radius + b.radius) return false;
    }
    return true;
  });
  const add = (i: number, d: Disc) => {
    out[i] = d;
    visit(d, (x, y) => {
      let column = buckets.get(x);
      if (!column) buckets.set(x, column = new Map());
      const list = column.get(y);
      if (list) list.push(i);
      else column.set(y, [i]);
      return true;
    });
  };
  discs.forEach((d, i) => { if (fixed?.[i]) add(i, d); });
  discs.forEach((wanted, i) => {
    if (fixed?.[i]) return;
    let d = constrain({ ...wanted });
    let attempt = 0;
    while (!clear(d)) {
      const local = attempt++ < 32;
      const k = local ? attempt : ++fallback;
      const angle = k * Math.PI * (3 - Math.sqrt(5));
      const distance = Math.sqrt(k) * (local ? Math.max(cell, wanted.radius * 2) : cell * 2);
      if (!local && bounds) {
        const row = Math.floor(k / columns);
        d = constrain({ ...wanted,
          x: bounds.left + ((k % columns) + 0.5) * ((bounds.right - bounds.left) / columns),
          y: bounds.top === undefined ? middle + Math.ceil(row / 2) * (row % 2 ? -pitch : pitch) : bounds.top - pitch / 2 - row * pitch,
        });
      } else d = constrain({ ...wanted, x: (local ? wanted.x : 0) + Math.cos(angle) * distance, y: (local ? wanted.y : 0) + Math.sin(angle) * distance });
    }
    add(i, d);
  });
  return out;
}

/** A reserved radius includes room beyond a glyph. The renderer projects it to screen space before drawing,
 * since Sigma's nominal glyph sizes are pixels, not layout units. */
export function clearanceRadius(size: number, spacing: number): number {
  return Math.max(1, size) * (1.5 + spacing);
}

/** Cache projection sizes between reducer passes; pan-only frames do no work, zoom/resize settles once. */
export function sizeRefresh(read: () => readonly [number, number], refresh: () => void) {
  let value = read(), target = value;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const same = (a: readonly number[], b: readonly number[]) => a.every((v, i) => Math.abs(v - b[i]) <= 1e-8 * Math.max(1, Math.abs(v), Math.abs(b[i])));
  const changed = () => {
    const next = read();
    if (same(next, target)) return;
    target = next;
    clearTimeout(timer);
    timer = undefined;
    if (same(next, value)) return;
    timer = setTimeout(() => {
      timer = undefined;
      const next = read();
      target = next;
      if (same(next, value)) return;
      value = next;
      refresh();
    }, 80);
  };
  return { value: () => value, changed, dispose: () => clearTimeout(timer) };
}
