// The spine view's reads (Jordan's brief part 2): a graph's declared spines (`umx graph spine show`) and where one of
// them places each node (`umx node list --columns id,spine:<name>`), both read through umx, so the view lays a graph
// out by umx's own placement rule. Each read checks the graph against the index first, as a snapshot does, so the
// view never makes umx load a graph it may not open.
import { graphIds } from "@/lib/snapshot";
import { PAGE, type createUmx } from "@/lib/umx";
import type { SpineInfo, SpinePlaces } from "@/lib/types";

type Reader = ReturnType<typeof createUmx>;

/** The most placements one read takes, as the snapshot's node cap. */
const CAP = 100_000;
const BY = new Set(["field", "link", "evidence", "rule", "not_placed"]);

const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);

/** The graph's declared spines, the main one first, then as umx lists them. */
export async function listSpines(read: Reader, graph: string): Promise<SpineInfo[]> {
  await graphIds(read, graph);
  const env = await read<{ spines?: unknown }>("spine show", { graph });
  const rows = Array.isArray(env.data?.spines) ? (env.data.spines as Record<string, unknown>[]) : [];
  const out: SpineInfo[] = [];
  for (const r of rows) {
    const decl = (r.decl ?? {}) as Record<string, unknown>;
    const name = str(decl.name);
    if (!name || !/^[a-z][a-z0-9_]{0,63}$/.test(name)) continue;
    const cover = (r.cover ?? {}) as Record<string, unknown>;
    const anchors = (decl.anchors ?? {}) as Record<string, unknown>;
    out.push({
      name,
      title: str(decl.title) ?? name,
      main: decl.main === true,
      clock: str(r.clock) === "calendar" ? "calendar" : "order",
      anchorType: str(anchors.type) ?? "",
      cover: {
        total: num(cover.total),
        byField: num(cover.by_field),
        byLink: num(cover.by_link),
        byEvidence: num(cover.by_evidence),
        byRule: num(cover.by_rule),
        notPlaced: num(cover.not_placed),
      },
    });
  }
  return out.sort((a, b) => Number(b.main) - Number(a.main));
}

/** Where spine `name` places each node of `graph`: [id, band (null when not placed), how, anchor (1) or not (0)]. A
 * row umx gave no cell for (an older umx, or a spine since removed) is left out. */
export async function loadSpinePlaces(read: Reader, graph: string, name: string): Promise<SpinePlaces> {
  await graphIds(read, graph);
  const column = `spine:${name}`;
  const places: SpinePlaces["places"] = [];
  let total = 0;
  let cells = 0;
  for (let offset = 0; offset < CAP; offset += PAGE) {
    const env = await read<{ rows: Record<string, unknown>[]; total: number }>("spine places", { graph, spine: name, offset });
    total = env.data.total;
    for (const r of env.data.rows) {
      const id = str(r.id);
      const cell = r[column] as { band?: unknown; by?: unknown; anchor?: unknown } | null | undefined;
      if (!id || !cell || typeof cell !== "object") continue;
      cells++;
      const by = str(cell.by);
      const band = typeof cell.band === "number" && Number.isSafeInteger(cell.band) ? cell.band : null;
      places.push([id, band, by && BY.has(by) ? by : "not_placed", cell.anchor === true ? 1 : 0]);
    }
    if (env.data.rows.length < PAGE || offset + PAGE >= total) break;
  }
  return { graph, spine: name, places, supported: total === 0 || cells > 0 };
}
