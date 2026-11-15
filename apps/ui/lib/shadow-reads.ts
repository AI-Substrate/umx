// Who owns a shadow read (ws009 M1c; review B2 to B4): one read at a time, each with its own generation and
// abort signal. A new read, a new open set or Off aborts the one in flight, and a reply for anything but the newest read
// is dropped, so an old answer never lands over a newer one, even A to B to A. Refresh always asks the server, which
// reads the pairs again whenever a graph's vectors_digest moved (lib/shadows-read, plan 184). While some
// open graphs hold changes not applied (Hold), only the others are read, and the drawn pairs that touch a held graph
// stay as they were, so shadows always match what the canvas shows.
import { indexPairs, mergeShadowReads, pairsOf, type Pair, type ShadowIndex } from "@/lib/graph/shadows";
import type { ShadowGroup, ShadowReply } from "@/lib/shadows-read";

export type ShadowData = {
  scope: string;
  pairs: Pair[];
  index: ShadowIndex;
  groups: ShadowGroup[];
  skipped: ShadowReply["skipped"];
  tooLarge: boolean;
  cut: boolean;
};
export type ShadowState = {
  scope: string | null;
  data: ShadowData | null;
  loading: boolean;
  error: string | null;
  /** Open graphs with held changes, not read again until they are applied. */
  waiting: string[];
  /** How long the last read took, in ms. */
  ms: number | null;
};
export const NO_SHADOWS: ShadowState = { scope: null, data: null, loading: false, error: null, waiting: [], ms: null };
export type Fetcher = (graphs: string[], signal: AbortSignal) => Promise<ShadowReply>;

export class ShadowReads {
  private scope: string | null = null;
  private open: string[] = [];
  private held = new Set<string>();
  private heldKey = "";
  private on = false;
  private gen = 0;
  private ac: AbortController | null = null;
  state: ShadowState = NO_SHADOWS;

  constructor(
    private readonly fetcher: Fetcher,
    private readonly publish: (s: ShadowState) => void,
    private readonly clock: () => number = () => performance.now(),
  ) {}

  /** What is on screen now: the open set's key, its graphs, the ones holding changes, and whether shadows are on. A
   * new open set drops everything and reads; Off stops any read and drops everything. */
  update(scope: string | null, open: readonly string[], held: Iterable<string>, on: boolean) {
    this.open = [...open];
    this.held = new Set(held);
    const heldKey = [...this.held].sort().join(",");
    const heldChanged = heldKey !== this.heldKey;
    this.heldKey = heldKey;
    if (!on || scope === null) {
      const had = this.on || this.state.data !== null || this.state.loading;
      this.on = false;
      this.scope = scope;
      this.cancel();
      if (had) this.set({ ...NO_SHADOWS, scope });
      return;
    }
    const fresh = scope !== this.scope || !this.on;
    this.on = true;
    if (!fresh) {
      // A graph began or stopped holding changes while a read runs (review N2): that read may hold pairs the
      // canvas has not applied, so it is dropped and asked again without the held graphs.
      if (heldChanged && this.ac !== null) this.read();
      return;
    }
    this.scope = scope;
    this.cancel();
    this.set({ ...NO_SHADOWS, scope });
    this.read();
  }

  /** Read again now (the refresh button, a live change, an Apply). Always asks the server; it answers from what it
   * kept only while every graph's vectors_digest stands (plan 184). */
  refresh() {
    if (this.on && this.scope !== null) this.read();
  }

  dispose() {
    this.cancel();
  }

  private cancel() {
    this.gen++;
    this.ac?.abort();
    this.ac = null;
  }

  private read() {
    this.cancel();
    const gen = this.gen;
    const scope = this.scope!;
    const ac = new AbortController();
    this.ac = ac;
    const graphs = this.open.filter((g) => !this.held.has(g));
    const waiting = this.open.filter((g) => this.held.has(g));
    if (graphs.length === 0) {
      this.set({ ...this.state, loading: false, waiting });
      return;
    }
    this.set({ ...this.state, loading: true, error: null, waiting });
    const t0 = this.clock();
    const mine = () => gen === this.gen && !ac.signal.aborted;
    this.fetcher(graphs, ac.signal).then(
      (reply) => {
        if (!mine()) return;
        this.ac = null;
        const old = this.state.data?.scope === scope ? this.state.data.pairs : [];
        const pairs = mergeShadowReads(old, pairsOf(reply.rows, reply.groups), new Set(graphs));
        const data: ShadowData = { scope, pairs, index: indexPairs(pairs), groups: reply.groups, skipped: reply.skipped, tooLarge: reply.tooLarge === true, cut: reply.cut === true };
        this.set({ scope, data, loading: false, error: null, waiting, ms: Math.round(this.clock() - t0) });
      },
      (e: unknown) => {
        if (!mine()) return;
        this.ac = null;
        this.set({ ...this.state, loading: false, error: e instanceof Error ? e.message : "The shadows could not be read." });
      },
    );
  }

  private set(s: ShadowState) {
    this.state = s;
    this.publish(s);
  }
}
