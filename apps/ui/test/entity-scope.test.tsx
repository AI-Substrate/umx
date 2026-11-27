// Merged entities in the panel, Find and Centre on this (PRD 005 centre-14 to centre-16).
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { type HitEntities, HitList, hitRows } from "@/components/hit-list";
import { NodePanel } from "@/components/node-panel";
import { neighbourLinks } from "@/lib/graph/build";
import { forcedLabels } from "@/lib/graph/edges";
import { aliasOfKey } from "@/lib/graph/keys";
import { readMembers, unionNeighbours } from "@/lib/entity-links";
import type { Hit, Neighbour, NodeDetail } from "@/lib/types";
import { MultiDirectedGraph } from "graphology";
import type { EdgeAttrs, NodeAttrs } from "@/lib/graph/build";

const row = (id: string, graph: string, edge: string, more: Partial<Neighbour> = {}): Neighbour =>
  ({ kind: "node", id, graph, graphAlias: null, local: true, way: "out", relation: "about", edge, type: "thing", title: id, status: "open", ...more }) as Neighbour;
const detail = (id: string, neighbours: Neighbour[], truncated = false): NodeDetail => ({ node: { id, title: id, type: "person", status: "open" }, neighbours, neighboursTruncated: truncated });

describe("a merged entity's links (centre-14)", () => {
  const open = new Map([["gid-a", "a"], ["gid-b", "b"]]);
  const linksOf = (m: string) => neighbourLinks(open, aliasOfKey(m), new Set());
  it("lists every member's neighbours once, each mapped from its own graph, and leaves out links between members", () => {
    const paulA = detail("P1", [row("X", "gid-a", "e1"), row("P2", "gid-b", "same", { local: false, relation: "same_as" })], true);
    const paulB = detail("P2", [row("Y", "gid-b", "e2"), row("P1", "gid-a", "same", { local: false, relation: "same_as", way: "in" })]);
    const got = unionNeighbours([{ key: "a/P1", detail: paulA }, { key: "b/P2", detail: paulB }, { key: "a/P3", detail: null }], new Set(["a/P1", "b/P2"]), linksOf);
    expect(got.neighbours.map((n) => got.links.target(n))).toEqual(["a/X", "b/Y"]);
    expect([got.truncated, got.failed]).toEqual([true, 1]);
  });

  it("reads members a few at a time and counts a refused read as null", async () => {
    let running = 0, most = 0;
    const read = async (key: string) => {
      most = Math.max(most, ++running);
      await new Promise((r) => setTimeout(r, 1));
      running--;
      if (key === "a/bad") throw new Error("node_not_found");
      return detail(key, []);
    };
    const keys = ["a/1", "a/2", "a/bad", "a/4", "a/5", "a/6", "a/7"];
    const got = await readMembers(keys, read, 3);
    expect(got.map((p) => p.detail === null)).toEqual([false, false, true, false, false, false, false]);
    expect(most).toBe(3);
  });

  it("labels every member's edges as the selection's on the canvas", () => {
    const g = new MultiDirectedGraph<NodeAttrs, EdgeAttrs>();
    for (const k of ["a/P1", "b/P2", "a/X", "b/Y"]) g.addNode(k);
    g.addEdgeWithKey("a/e1", "a/P1", "a/X");
    g.addEdgeWithKey("b/e2", "b/P2", "b/Y");
    const at = { selected: "a/P1", hovered: null, focusCentre: null };
    expect([...forcedLabels(g, at, () => true).edges]).toEqual(["a/e1"]);
    expect([...forcedLabels(g, { ...at, members: ["a/P1", "b/P2"] }, () => true).edges]).toEqual(["a/e1", "b/e2"]);
  });

  it("titles the panel's neighbours as all members', and says while the others are read or when some fail", () => {
    const panel = (members: { count: number; failed: number; loading: boolean }) => renderToStaticMarkup(
      <NodePanel open detail={detail("P1", [])} error={null} loading={false} theme="light" onClose={() => {}} onPick={() => {}} hiddenIds={() => false}
        focus={{ depth: 0, focus: { state: "off" }, onDepth: () => {} }} members={members} />,
    );
    expect(panel({ count: 3, failed: 0, loading: true })).toContain("Neighbours of all 3 members (0)");
    expect(panel({ count: 3, failed: 0, loading: true })).toContain("members-loading");
    expect(panel({ count: 3, failed: 1, loading: false })).toContain("The links of 1 member could not be read.");
  });
});

describe("Find: one row per entity (centre-15)", () => {
  const hit = (id: string): Hit => ({ id, type: "person", title: id, status: "open", score: 1, legs: null, exact: null });
  const members = [{ key: "a/P1", graph: "a", title: "Paul" }, { key: "b/P2", graph: "b", title: "Paul" }, { key: "c/P3", graph: "c", title: "Paul" }];
  const entityOf: HitEntities = (id) => (id.endsWith("/P1") || id.endsWith("/P2") ? { entity: 0, members } : null);

  it("lists an entity once, at its best hit, and every other hit as before", () => {
    const rows = hitRows([hit("b/P2"), hit("a/X"), hit("a/P1")], entityOf);
    expect(rows.map((r) => [r.hit.id, r.entity])).toEqual([["b/P2", 0], ["a/X", null]]);
    expect(hitRows([hit("b/P2"), hit("a/P1")], null).map((r) => r.hit.id)).toEqual(["b/P2", "a/P1"]);
  });

  it("names the entity's graphs and offers its members", () => {
    const html = renderToStaticMarkup(
      <HitList result={{ mode: "title", query: "paul", hits: [hit("b/P2"), hit("a/P1")], warnings: [], note: null, foreign: 0 }} theme="light"
        place={() => ({ kind: "shown" })} onPick={() => {}} onReveal={() => {}} testId="t" entityOf={entityOf} onPickMember={() => {}} />,
    );
    expect(html.match(/data-id="[^"]*"/g)).toEqual(['data-id="b/P2"']);
    expect(html).toContain("a, b, c");
    expect(html).toContain('aria-label="Show members"');
  });
});

describe("Centre on this in the panel (centre-01, centre-08)", () => {
  const panel = (here: boolean) => renderToStaticMarkup(
    <NodePanel open detail={detail("P1", [])} error={null} loading={false} theme="light" onClose={() => {}} onPick={() => {}} hiddenIds={() => false}
      focus={{ depth: 0, focus: { state: "off" }, onDepth: () => {} }} centre={{ here, paused: false, onCentre: () => {}, onBack: () => {} }} />,
  );
  it("offers Centre on this, and Back to layout while centred on this node", () => {
    expect(panel(false)).toContain('data-testid="centre-on"');
    expect(panel(false)).toContain("Centre on this");
    expect(panel(true)).toContain('data-testid="centre-back"');
    expect(panel(true)).toContain("Back to layout");
  });
});
