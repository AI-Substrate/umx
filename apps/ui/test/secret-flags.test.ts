// umx's read-side secret flags as the view applies them (F188, usage 174; ws009 M2b, plan 176): each flagged value is
// replaced by position, in the reply's own arrays, and the flags never travel on as values.
import { describe, expect, it } from "vitest";
import { mergeRecords } from "@/lib/graph/entities";
import { HIDDEN_VALUE, redactNode } from "@/lib/secret-flags";

const S = "sk-live-0000000000000000000000000000";
const node = {
  id: "01JC8XA4H7AAAAAAAAAAQ7K2M9",
  type: "person",
  title: S,
  status: "open",
  note: S,
  phone: S,
  team: "ops",
  locator: { source: S },
  entity_key: [7, "email:jo@example.invalid", `token:${S}`, `Bad-Scheme:${S}`],
  evidence: [{ source: "https://example.invalid/ok", kind: "web" }, { source: S, kind: "web", selector: { quote: S } }],
  status_history: [{ from: "open", to: "done", reason: S }, { from: "done", to: "open", reason: "fine" }],
  evidence_removed: [{ removed: S, reason: S, at: "2026-11-12" }],
  secret_suspected: ["field:title", "field:note", "field:phone", "field:locator", "key:3", "key:4", "evidence:2", "status_reason:1", "evidence_removed:1"],
};

describe("redactNode", () => {
  it("replaces each flagged value by its place, keeps every other value, and drops the flags", () => {
    const { node: out, titleHidden } = redactNode(node);
    expect(titleHidden).toBe(true);
    expect(out).toEqual({
      id: node.id,
      type: "person",
      title: `person · ${node.id.slice(-6)}`,
      status: "open",
      note: HIDDEN_VALUE,
      phone: HIDDEN_VALUE,
      team: "ops",
      locator: HIDDEN_VALUE,
      // key:<i> counts the reply's own array from 1 (F217), a non-string entry included.
      entity_key: [7, "email:jo@example.invalid", "token:(hidden key)", "(hidden key)"],
      evidence: [node.evidence[0], { source: HIDDEN_VALUE, kind: "web" }],
      status_history: [{ from: "open", to: "done", reason: HIDDEN_VALUE }, node.status_history[1]],
      evidence_removed: [{ removed: HIDDEN_VALUE, reason: HIDDEN_VALUE, at: "2026-11-12" }],
    });
    expect(JSON.stringify(out)).not.toContain(S);
    // The input is left as it was.
    expect(node.entity_key[2]).toBe(`token:${S}`);
  });

  it("hides a whole entity_key that is not a list when it is flagged as a field", () => {
    expect(redactNode({ id: "x", type: "t", entity_key: `token:${S}`, secret_suspected: ["field:entity_key"] }).node.entity_key).toBe("(hidden key)");
  });

  it("fails closed on a flag it does not know: every field that holds graph text is hidden", () => {
    const { node: out, titleHidden } = redactNode({ id: "01AAAAAAAAAAAAAAAAAAAAAAAA", type: "t", status: "open", title: S, team: S, secret_suspected: ["shape:0"] });
    expect(out).toEqual({ id: "01AAAAAAAAAAAAAAAAAAAAAAAA", type: "t", status: "open", title: "t · AAAAAA", team: HIDDEN_VALUE });
    expect(titleHidden).toBe(true);
  });

  it("fails closed on a list position 0, which names no entry from 1 (F217)", () => {
    const { node: out } = redactNode({ id: "x", type: "t", team: S, entity_key: [`token:${S}`], secret_suspected: ["key:0"] });
    expect(out).toEqual({ id: "x", type: "t", team: HIDDEN_VALUE, entity_key: HIDDEN_VALUE });
  });

  it("changes nothing but the flags field when nothing is flagged", () => {
    const plain = { id: "x", type: "t", title: "Jo", entity_key: ["email:jo@example.invalid"] };
    expect(redactNode({ ...plain, secret_suspected: [] })).toEqual({ node: plain, titleHidden: false });
  });

  it("never merges two hidden values into one", () => {
    const merged = mergeRecords([
      { graph: "a", key: "a/1", node: { phone: HIDDEN_VALUE, updated: "2026-11-02" } },
      { graph: "b", key: "b/1", node: { phone: HIDDEN_VALUE, updated: "2026-11-01" } },
    ])!;
    expect(merged.fields[0]).toMatchObject({ name: "phone", value: HIDDEN_VALUE, others: [{ value: HIDDEN_VALUE, graph: "b" }] });
  });
});

// ws008 TM6: a status move stores one evidence pointer, and `node get` flags a suspect one as
// `status_evidence:<i>` (plan 192).
describe("status_evidence:<i>", () => {
  const P = "notes/creds.md#sk-live-1111111111111111111111111111";
  const move = <T extends object>(from: string, to: string, extra: T = {} as T) => ({ at: "2026-11-13T07:00:00Z", by: "jo", from, to, ...extra });
  const moved = {
    id: "01JC8XA4H7AAAAAAAAAAQ7K2M9",
    type: "decision",
    title: "Ship the pilot",
    status: "agreed",
    evidence: [{ source: "docs/brief.md#L10-L24", kind: "document" }],
    status_history: [
      move("open", "proposed", { reason: "raised", evidence: { source: "calls/a.vtt#seq=1-2", kind: "room", by_session: "01SESSIONAAAAAAAAAAAAAAAAA" } }),
      move("proposed", "agreed", {
        reason: "Signed off in the review",
        in: "umx:01GRAPHAAAAAAAAAAAAAAAAAAA/01NODEAAAAAAAAAAAAAAAAAAAA",
        evidence: { source: P, kind: "room", selector: { quote: P }, hash: "sha256:00", by_session: "01SESSIONBBBBBBBBBBBBBBBBBB" },
      }),
    ],
  };

  it("hides only that move's evidence as a new { source, kind }, keeping move 0, the reason, the title and node evidence", () => {
    const { node: out, titleHidden } = redactNode({ ...moved, secret_suspected: ["status_evidence:2"] });
    expect(titleHidden).toBe(false);
    expect(out.title).toBe("Ship the pilot");
    expect(out.evidence).toEqual(moved.evidence);
    const history = out.status_history as Record<string, unknown>[];
    expect(history[0]).toEqual(moved.status_history[0]);
    // Selector, hash and by_session are dropped with the source: exactly { source, kind } (security on plan 192).
    expect(history[1]).toEqual({ ...moved.status_history[1], evidence: { source: HIDDEN_VALUE, kind: "room" } });
    expect(JSON.stringify(out).includes(P)).toBe(false);
    expect(JSON.stringify(out)).not.toContain("01SESSIONBBBBBBBBBBBBBBBBBB");
    // The input is left as it was.
    expect(moved.status_history[1].evidence.source).toBe(P);
  });

  it("drops a kind that is not a string", () => {
    const odd = { ...moved, status_history: [move("open", "agreed", { evidence: { source: P, kind: 3 } })], secret_suspected: ["status_evidence:1"] };
    expect((redactNode(odd).node.status_history as Record<string, unknown>[])[0].evidence).toEqual({ source: HIDDEN_VALUE });
  });

  it("applies status_reason and status_evidence on the same move", () => {
    const { node: out } = redactNode({ ...moved, secret_suspected: ["status_reason:2", "status_evidence:2"] });
    const history = out.status_history as Record<string, unknown>[];
    expect(history[1]).toMatchObject({ reason: HIDDEN_VALUE, evidence: { source: HIDDEN_VALUE, kind: "room" } });
    expect(history[0]).toEqual(moved.status_history[0]);
  });

  it("changes nothing for an index past the last move or a move with no evidence", () => {
    const plain = { ...moved };
    expect(redactNode({ ...moved, secret_suspected: ["status_evidence:3"] })).toEqual({ node: plain, titleHidden: false });
    const bare = { ...moved, status_history: [move("open", "agreed")] };
    expect(redactNode({ ...bare, secret_suspected: ["status_evidence:1"] })).toEqual({ node: bare, titleHidden: false });
  });
});
