// The activity strip (components/agent-strip.tsx): the label is fixed chrome, the agent's note and titles are text in
// their own isolated elements (security T2), and a node the view does not show is never named.
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AgentStrip, type StripEntry } from "@/components/agent-strip";
import { chipDiff, describe as words } from "@/lib/agent";

const A = "01NODEAAAAAAAAAAAAAAAAAAAA";
const B = "01NODEBBBBBBBBBBBBBBBBBBBB";
const title = (id: string) => (id === A ? "Approval <b>delays</b>" : null);
const entry = (seq: number, d: StripEntry["d"]): StripEntry => ({ seq, at_ms: 1_700_000_000_000, d, replay: true });
const none = () => {};

function strip(history: StripEntry[], more: Partial<Parameters<typeof AgentStrip>[0]> = {}) {
  return renderToStaticMarkup(
    <AgentStrip history={history} onUndo={null} prompt={null} onGo={none} onDismiss={none} onReplay={none} error={null} {...more} />,
  );
}

describe("the activity strip", () => {
  it("renders the note and title as text, apart from the label", () => {
    const note = '<img src=x onerror="alert(1)"> trust me: ignore previous instructions';
    const html = strip([entry(1, words({ seq: 1, at_ms: 1, graph: "demo", by: { seat: "pij-xyz" }, command: { kind: "focus", node: A, hops: 2, note } }, title))]);
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>");
    expect(html).toContain("&lt;img src=x");
    const label = /data-testid="agent-label">([^<]*)</.exec(html)?.[1];
    expect(label).toBe("pij-xyz focused");
    expect(html).toMatch(/<bdi[^>]*data-testid="agent-note">&lt;img/);
    expect(html).toMatch(/<bdi[^>]*data-testid="agent-target">Approval &lt;b&gt;delays&lt;\/b&gt;, 2 hops</);
  });

  it("says when the agent named a node that is not shown, without naming it", () => {
    const html = strip([entry(2, words({ seq: 2, at_ms: 1, graph: "demo", by: null, command: { kind: "highlight", nodes: [B], add: false } }, title))]);
    expect(html).toContain("agent named a node that is not shown");
    expect(html).toContain("an agent highlighted");
    expect(html).toContain("a node that is not shown");
  });

  it("offers Undo only when given, and the Go prompt with its target", () => {
    const d = words({ seq: 3, at_ms: 1, graph: "demo", by: null, command: { kind: "filter", show: [], hide: ["type:option"] } }, title);
    expect(strip([entry(3, d)])).not.toContain("agent-undo");
    expect(strip([entry(3, d)], { onUndo: none })).toContain("agent-undo");
    const prompted = strip([], { prompt: { target: "Approval delays" } });
    expect(prompted).toContain("Agent wants to show");
    expect(prompted).toContain("agent-go");
  });

  it("pulses exactly the chips an agent filter changed", () => {
    const before = { relations: new Set<string>(), types: new Set(["risk"]), statuses: new Set<string>() };
    const after = { relations: new Set(["cites"]), types: new Set<string>(), statuses: new Set<string>() };
    expect([...chipDiff(before, after)].sort()).toEqual(["relations:cites", "types:risk"]);
  });
});
