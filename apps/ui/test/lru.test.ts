// The canvas's label caches (lib/lru.ts, reviewer R370-3): bounded by entries, least recently used dropped
// first, and an entry dropped is made again when it is asked for.
import { describe, expect, it } from "vitest";
import { Lru } from "@/lib/lru";

describe("a bounded label cache", () => {
  it("holds at most its limit, dropping the least recently used, and a read counts as a use", () => {
    const cache = new Lru<string, number>(3);
    for (const [k, v] of [["a", 1], ["b", 2], ["c", 3]] as const) cache.set(k, v);
    expect(cache.get("a")).toBe(1);
    cache.set("d", 4);
    expect(cache.size).toBe(3);
    expect([cache.get("b"), cache.get("a"), cache.get("c"), cache.get("d")]).toEqual([undefined, 1, 3, 4]);
    for (let i = 0; i < 100; i++) cache.set(`k${i}`, i);
    expect(cache.size).toBe(3);
  });

  it("makes a dropped label again when it comes back, and makes a held one only once", () => {
    const cache = new Lru<string, { text: string; made: number }>(2);
    let made = 0;
    const image = (text: string) => cache.getOr(text, () => ({ text, made: ++made }));
    const first = image("supports ›");
    expect(image("supports ›")).toBe(first);
    image("‹ supports");
    image("blocks ›");
    const again = image("supports ›");
    expect(again).not.toBe(first);
    expect(again).toEqual({ text: "supports ›", made: 4 });
    expect(made).toBe(4);
  });
});
