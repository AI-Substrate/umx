// The node panel's polish (ws009 S8, plan 181): Console's loading skeleton (ui-expert K1-N5) and ids that never wrap
// mid-token (K1-N6).
import { renderToStaticMarkup } from "react-dom/server";
import type { ReactElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NeighbourList, NodePanel } from "@/components/node-panel";
import { NodeSpines } from "@/components/node-spines";
import type { NodeOnSpine } from "@/lib/graph/spine-layout";
import { copyText, IdText, idLike, Value } from "@/components/text";
import type { Theme } from "@/lib/graph/colors";
import { HIDDEN_VALUE } from "@/lib/secret-flags";
import { loadNode } from "@/lib/snapshot";
import { createUmx, UMX_PROTOCOL, type Exec } from "@/lib/umx";
import type { Neighbour } from "@/lib/types";

const ID = "01M78ADDVW6AJP6E2SM22WP9KE";

it("shows held edge attribution without treating manual members as auto-added", () => {
  const member: Neighbour = { kind: "node", id: ID, graph: "gid", graphAlias: "demo", local: true, way: "in", relation: "part_of", edge: "manual", type: "risk", title: "Manual member", status: "open" };
  const html = renderToStaticMarkup(<NeighbourList rows={[member, { ...member, edge: "auto", title: "Auto member" }]} theme="light" onPick={() => {}} hiddenIds={() => false} links={{
    target: () => `demo/${ID}`, graphName: () => "demo", edgeOrigin: (n) => n.edge === "auto" ? "risk_register" : null,
  }} />);
  expect(html).toContain("Manual member");
  expect(html).toContain("Auto member");
  expect(html.match(/auto_added_by/g)).toHaveLength(1);
  expect(html).toContain("risk_register");
});

function panel(theme: Theme, loading: boolean, node?: Record<string, unknown>) {
  return renderToStaticMarkup(
    <NodePanel
      open
      detail={node ? { node, neighbours: [], neighboursTruncated: false } : null}
      error={null}
      loading={loading}
      theme={theme}
      onClose={() => {}}
      onPick={() => {}}
      hiddenIds={() => false}
      focus={{ depth: 0, focus: { state: "off" }, onDepth: () => {} }}
    />,
  );
}

describe("Console reads a node with a still skeleton, the other themes with the spinner (K1-N5)", () => {
  it("draws a fixed skeleton in Console, never the spin animation", () => {
    const html = panel("console", true);
    expect(html).toContain('data-testid="panel-skeleton"');
    expect(html).not.toContain("animate-spin");
    expect(html).not.toContain("animate-pulse");
    const light = panel("light", true);
    expect(light).toContain("animate-spin");
    expect(light).not.toContain("panel-skeleton");
  });
});

describe("ids and addresses never wrap mid-token (K1-N6)", () => {
  it("draws an id in mono on one line, cut with an ellipsis, with a copy button, and leaves other text as it was", () => {
    expect([idLike(ID), idLike(`umx:crm/${ID}`), idLike("Launch date"), idLike(ID.toLowerCase())]).toEqual([true, true, false, false]);
    const id = renderToStaticMarkup(<Value value={ID} />);
    expect(id).toContain("font-mono");
    expect(id).toContain("truncate");
    expect(id).not.toContain("break-");
    expect(id).toContain('data-testid="copy-id"');
    const text = renderToStaticMarkup(<Value value="Launch date" />);
    expect(text).toContain("break-words");
    expect(text).not.toContain("copy-id");
  });

  it("uses it for the panel's address, record id and an id field", () => {
    const html = panel("light", false, { id: ID, type: "problem", title: "T", status: "open", address: `umx:crm/${ID}`, authored_by_session: ID });
    expect(html.match(/data-testid="id-text"/g)).toHaveLength(3);
    expect(html).not.toContain("break-all");
  });
});

describe("the copy button puts the whole id on the clipboard (reviewer N2)", () => {
  afterEach(() => vi.unstubAllGlobals());
  type Props = { children?: ReactElement<Props>[]; onClick?: () => void; "data-testid"?: string };
  const click = (value: string) => {
    const root = IdText({ value }) as ReactElement<Props>;
    const button = root.props.children!.find((c) => c.props["data-testid"] === "copy-id")!;
    button.props.onClick!();
  };
  const settle = () => new Promise((r) => setTimeout(r, 0));

  it("copies the full shown value on a click", async () => {
    const writeText = vi.fn(async (value: string) => void value);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    click(`umx:crm/${ID}`);
    await settle();
    expect(writeText).toHaveBeenCalledWith(`umx:crm/${ID}`);
  });

  it("does not throw or leave a rejection when the clipboard refuses or is missing", async () => {
    // A plain function, not vi.fn: a spy handles the promises it returns, which would hide the rejection.
    const asked: string[] = [];
    const refused = async (value: string) => {
      asked.push(value);
      throw new DOMException("Document is not focused.", "NotAllowedError");
    };
    const unhandled = vi.fn();
    process.on("unhandledRejection", unhandled);
    try {
      vi.stubGlobal("navigator", { clipboard: { writeText: refused } });
      expect(() => click(ID)).not.toThrow();
      await settle();
      expect(asked).toEqual([ID]);
      await expect(copyText(ID)).resolves.toBe(false);
      vi.stubGlobal("navigator", {});
      expect(() => click(ID)).not.toThrow();
      await expect(copyText(ID)).resolves.toBe(false);
      await settle();
      expect(unhandled).not.toHaveBeenCalled();
    } finally {
      process.off("unhandledRejection", unhandled);
    }
  });
});

// A status move's evidence pointer (ws008 TM6; plan 192), drawn under the move's reason as the Evidence section
// draws a node's.
describe("a status move's evidence in Status history (plan 192)", () => {
  const P = "notes/creds.md#sk-live-1111111111111111111111111111";
  const IN = "umx:01GRAPHZZZZZZZZZZZZZZZZZZZ/01NODEZZZZZZZZZZZZZZZZZZZZ";
  const GA = "01GRAPHAAAAAAAAAAAAAAAAAAA";
  const move = (extra: object) => ({ at: "2026-11-13T07:00:00Z", by: "jo", from: "proposed", to: "agreed", reason: "Signed off", ...extra });
  const node = (history: object[]) => ({ id: ID, type: "decision", title: "Ship the pilot", status: "agreed", status_history: history });
  const moveEvidence = (html: string) => html.slice(html.indexOf('data-testid="move-evidence"'));

  it("draws the source and kind, a umx: pointer as plain text with no link, and never the move's in anchor", () => {
    const html = panel("light", false, node([move({ in: IN, evidence: { source: "calls/review.vtt#seq=40-52", kind: "room" } }), move({ evidence: { source: `umx:crm/${ID}`, kind: "node" } })]));
    const shown = moveEvidence(html);
    expect(shown).toContain("calls/review.vtt#seq=40-52");
    expect(shown).toContain(">room<");
    expect(shown).toContain(`umx:crm/${ID}`);
    expect(shown).toContain(">node<");
    expect(shown).not.toContain("<a ");
    expect(html).not.toContain(IN);
  });

  it("draws a redacted entry as (redacted) with its kind, never another of its fields", () => {
    const redacted = { kind: "room", redacted: true, removed_at: "2026-11-13T08:00:00Z", by: "jo", by_session: "01SESSIONCCCCCCCCCCCCCCCCCC", reason: "the path names a credential" };
    const html = panel("light", false, node([move({ evidence: redacted })]));
    const shown = moveEvidence(html);
    expect(shown).toContain("(redacted)");
    expect(shown).toContain(">room<");
    for (const text of ["the path names a credential", "01SESSIONCCCCCCCCCCCCCCCCCC", "2026-11-13T08:00:00Z"]) expect(html).not.toContain(text);
  });

  it("draws a flagged pointer as hidden from loadNode's reply, never the original, with the title still shown", async () => {
    const env = (data: unknown) => JSON.stringify({ umx: UMX_PROTOCOL, command: "x", status: "ok", data, warnings: [], errors: [] });
    const replies: Record<string, string> = {
      "graph list": env({ rows: [{ alias: "alpha", id: GA, nodes: 1, edges: 0, state: "loaded", folder: "present", policy: "open" }], total: 1, truncated: false }),
      "node get": env({ node: { ...node([move({ evidence: { source: P, kind: "room", selector: { quote: P } } })]), secret_suspected: ["status_evidence:1"] } }),
      neighbours: env({ rows: [], total: 0, truncated: false }),
    };
    const exec = vi.fn<Exec>(async (_bin, args) => ({ stdout: replies[args[0] === "neighbours" ? "neighbours" : `${args[0]} ${args[1]}`], failed: false, detail: "" }));
    const detail = await loadNode(createUmx({ exec, bin: () => "/x/umx", env: () => ({ UMX_HOME: "/h", UMX_OUTPUT: "json" }) }), "alpha", ID);
    const html = renderToStaticMarkup(
      <NodePanel open detail={detail} error={null} loading={false} theme="light" onClose={() => {}} onPick={() => {}} hiddenIds={() => false} focus={{ depth: 0, focus: { state: "off" }, onDepth: () => {} }} />,
    );
    expect(moveEvidence(html)).toContain(HIDDEN_VALUE);
    expect(moveEvidence(html)).toContain(">room<");
    expect(html).not.toContain(P);
    expect(html).not.toContain("sk-live-");
    expect(html).toContain("Ship the pilot");
  });
});

describe("the node panel's Spines block", () => {
  const row = (over: Partial<NodeOnSpine>): NodeOnSpine => ({ name: "days", title: "Days", clock: "calendar", main: true, shown: false, state: "ready", place: null, anchors: [], side: null, ...over });
  const anchor = (id: string) => ({ key: `g/${id}`, title: `Title ${id}` });
  const render = (rows: NodeOnSpine[] | null | "failed") => renderToStaticMarkup(<NodeSpines rows={rows} anchor={anchor} onToggle={() => {}} onPick={() => {}} />);

  it("lists each spine as a toggle with its kind, the anchor by title and how the node was placed, or not on it", () => {
    const html = render([
      row({ shown: true, place: { band: 2, by: "link", anchor: false }, anchors: ["a2"] }),
      row({ name: "steps", title: "Steps", clock: "order", main: false }),
    ]);
    expect(html.match(/data-testid="node-spine-toggle"/g)).toHaveLength(2);
    expect(html).toContain('aria-pressed="true"');
    expect(html).toContain('aria-pressed="false"');
    expect(html).toContain("lucide-calendar-days");
    expect(html).toContain("lucide-list-ordered");
    expect(html).toContain("Title a2");
    expect(html).toContain("by a link");
    expect(html).toContain("Not on this spine.");
    expect(html).not.toContain("node-spines-nowhere");
  });

  it("says in one line when the node is on none of them, and when the graph declares none", () => {
    const html = render([row({}), row({ name: "steps", clock: "order" })]);
    expect(html.match(/On none of its graph/g)).toHaveLength(1);
    expect(html).not.toContain("Not on this spine.");
    expect(html.match(/data-testid="node-spine-toggle"/g)).toHaveLength(2);
    expect(render([])).toContain("declares no spines");
  });

  it("says the spines could not be read when listing them failed, never that the graph has none", () => {
    const html = render("failed");
    expect(html).toContain("node-spines-failed");
    expect(html).toContain("could not be read");
    expect(html).not.toContain("declares no spines");
  });

  it("names an anchor as one, and a node outside every anchor's band by its side", () => {
    expect(render([row({ place: { band: 0, by: "field", anchor: true } })])).toContain("An anchor, by its own field");
    expect(render([row({ place: { band: -1, by: "evidence", anchor: false }, side: "before" })])).toContain("before the first anchor, by its evidence");
  });
});
