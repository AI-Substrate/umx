export type Disc = { x: number; y: number; radius: number };

/** Spatially packs discs near their preferred positions. The shared fallback cursor bounds work even when every
 * preferred position is identical. Callers supply stable order; no random source or wall clock decides a position. */
export function separateDiscs(discs: readonly Disc[], fixed?: Uint8Array): Disc[] {
  if (discs.length === 0) return [];
  const radii = discs.map((d) => d.radius).sort((a, b) => a - b);
  const cell = Math.max(1, radii[radii.length >> 1] * 2);
  const buckets = new Map<string, number[]>();
  const out: Disc[] = new Array(discs.length);
  let fallback = 0;
  const visit = (d: Disc, read: (key: string) => boolean) => {
    for (let x = Math.floor((d.x - d.radius) / cell); x <= Math.floor((d.x + d.radius) / cell); x++) {
      for (let y = Math.floor((d.y - d.radius) / cell); y <= Math.floor((d.y + d.radius) / cell); y++) {
        if (!read(`${x},${y}`)) return false;
      }
    }
    return true;
  };
  const clear = (d: Disc) => visit(d, (key) => (buckets.get(key) ?? []).every((i) => {
    const b = out[i];
    return Math.hypot(d.x - b.x, d.y - b.y) >= d.radius + b.radius;
  }));
  const add = (i: number, d: Disc) => {
    out[i] = d;
    visit(d, (key) => {
      const list = buckets.get(key);
      if (list) list.push(i);
      else buckets.set(key, [i]);
      return true;
    });
  };
  discs.forEach((d, i) => { if (fixed?.[i]) add(i, d); });
  discs.forEach((wanted, i) => {
    if (fixed?.[i]) return;
    let d = { ...wanted };
    let attempt = 0;
    while (!clear(d)) {
      const local = attempt++ < 32;
      const k = local ? attempt : ++fallback;
      const angle = k * Math.PI * (3 - Math.sqrt(5));
      const distance = Math.sqrt(k) * (local ? Math.max(cell, wanted.radius * 2) : cell * 2);
      d = { ...wanted, x: (local ? wanted.x : 0) + Math.cos(angle) * distance, y: (local ? wanted.y : 0) + Math.sin(angle) * distance };
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
