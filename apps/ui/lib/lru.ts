/**
 * A map that keeps at most `limit` entries, dropping the least recently used (reviewer R370-3: the canvas's
 * label width and label image caches). A dropped entry is simply made again by the caller on its next miss.
 */
export class Lru<K, V> {
  private readonly entries = new Map<K, V>();

  constructor(readonly limit: number) {}

  get size(): number {
    return this.entries.size;
  }

  /** The value for `key`, marked most recently used; undefined when it is not held. */
  get(key: K): V | undefined {
    const value = this.entries.get(key);
    if (value === undefined) return undefined;
    this.entries.delete(key);
    this.entries.set(key, value);
    return value;
  }

  /** Holds `value` as the most recently used, dropping the least recently used past the limit. */
  set(key: K, value: V): void {
    this.entries.delete(key);
    this.entries.set(key, value);
    while (this.entries.size > this.limit) this.entries.delete(this.entries.keys().next().value as K);
  }

  /** The value for `key`, made by `make` and held when it is not (again after it was dropped). */
  getOr(key: K, make: () => V): V {
    const held = this.get(key);
    if (held !== undefined) return held;
    const value = make();
    this.set(key, value);
    return value;
  }
}
