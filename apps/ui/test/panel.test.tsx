// The node panel's polish (ws009 S8, plan 181): Console's loading skeleton (ui-expert K1-N5) and ids that never wrap
// mid-token (K1-N6).
import { renderToStaticMarkup } from "react-dom/server";
import type { ReactElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NodePanel } from "@/components/node-panel";
import { copyText, IdText, idLike, Value } from "@/components/text";
import type { Theme } from "@/lib/graph/colors";
import { HIDDEN_VALUE } from "@/lib/secret-flags";
import { loadNode } from "@/lib/snapshot";
import { createUmx, UMX_PROTOCOL, type Exec } from "@/lib/umx";

const ID = "01M78ADDVW6AJP6E2SM22WP9KE";

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
