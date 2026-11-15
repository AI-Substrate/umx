// The node panel's entity read (ws009 M2, plan 165; review B2 and B5): members, proposed partners and records
// are kept to the open graphs whatever umx covered, and the merged record reads every member up to a stated limit.
import { describe, expect, it } from "vitest";
import { loadEntity } from "@/lib/entity-read";
import { UmxError } from "@/lib/umx";
import { entityScope, offerMembers, replyFor, scopeOfAsk } from "@/lib/entity-scope";
import { mergeRecords } from "@/lib/graph/entities";
import type { createUmx } from "@/lib/umx";

const id = (i: number) => `01${String(i).padStart(24, "0")}`;
type Reader = ReturnType<typeof createUmx>;
const env = (data: unknown) => ({ umx: 60, command: "", status: "ok" as const, data, warnings: [], errors: [] });

/** A reader standing in for umx: `entity show` answers `members` (as umx's wide default would, across every loaded
 * open graph), and `node get` answers each member's record. */
type Fake = { graph: string; id: string; updated?: string; extra?: Record<string, unknown>; keys?: string[]; flags?: string[]; title?: string };
function reader(members: Fake[], calls: [string, unknown][] = []) {
  const row = (m: Fake) => ({
    graph: m.graph,
    id: m.id,
    type: "person",
    title: m.title ?? `${m.graph} ${m.id.slice(-2)}`,
    entity_key: m.keys ?? [],
    matched: [{ by: "same_as", graph: m.graph }],
    ...(m.flags ? { secret_suspected: m.flags } : {}),
  });
  return (async (verb: string, args: Record<string, unknown> = {}) => {
    calls.push([verb, args]);
    if (verb === "graph list") return env({ rows: ["crm", "hr", "beta"].map((g) => ({ alias: g, id: `G${g}`, state: "loaded", folder: "present", policy: "open" })) });
    if (verb === "entity show" || verb === "entity show loaded") {
      return env({ members: members.map(row), proposed: [{ graph: "beta", id: id(99), type: "person", title: "b", with: `umx:crm/${id(1)}`, edge_graph: "beta" }] });
    }
    const m = members.find((x) => x.id === args.id && x.graph === args.graph)!;
    return env({ node: { id: m.id, type: "person", title: m.title ?? `${m.graph} ${m.id.slice(-2)}`, updated: m.updated ?? "2026-11-01T00:00:00Z", entity_key: ["k:secret"], ...m.extra } });
  }) as unknown as Reader;
}

describe("the entity read keeps to the open graphs (review B2)", () => {
  it("with one graph open, shows and reads nothing from another loaded graph, and counts it as not open", async () => {
    const calls: [string, unknown][] = [];
    const read = reader([{ graph: "crm", id: id(1) }, { graph: "beta", id: id(2), updated: "2026-11-12T00:00:00Z", extra: { phone: "9" } }], calls);
    const reply = await loadEntity(read, "crm", id(1), ["crm"], true);
    expect(reply.members.map((m) => m.graph)).toEqual(["crm"]);
    expect(reply.proposed).toEqual([]);
    expect(reply.records!.map((r) => r.graph)).toEqual(["crm"]);
    expect(reply.notOpen).toEqual([{ graph: "beta", count: 1 }]);
    expect(calls.filter(([v]) => v === "node get").map(([, a]) => (a as { graph: string }).graph)).toEqual(["crm"]);
    expect(JSON.stringify(reply)).not.toContain("k:secret");
  });
});

describe("the merged record reads every member (review B5; S8 lifts the cap)", () => {
  it("takes the title and a field only a 13th member has, when it is the newest", async () => {
    const members = Array.from({ length: 13 }, (_v, i) => ({ graph: i % 2 ? "hr" : "crm", id: id(i + 1), updated: `2026-11-${String(i + 1).padStart(2, "0")}T00:00:00Z`, extra: i === 12 ? { team: "only-13" } : {} }));
    const reply = await loadEntity(reader(members), "crm", id(1), ["crm", "hr"], true);
    expect(reply.records).toHaveLength(13);
    expect(reply.recordsLeftOut).toBeUndefined();
    const merged = mergeRecords(reply.records!.map((r) => ({ graph: r.graph, key: `${r.graph}/${r.id}`, node: r.node })))!;
    expect(merged).toMatchObject({ title: `crm ${id(13).slice(-2)}`, updated: "2026-11-13T00:00:00Z" });
    expect(merged.fields.find((f) => f.name === "team")).toMatchObject({ value: "only-13" });
  });

  it("reads all 150 members six at a time, so the newest, last, gives the title", async () => {
    const calls: [string, unknown][] = [];
    const members = Array.from({ length: 150 }, (_v, i) => ({ graph: "crm", id: id(i + 1), updated: i === 149 ? "2026-11-30T00:00:00Z" : "2026-11-01T00:00:00Z", extra: i === 149 ? { team: "only-150" } : {} }));
    const reply = await loadEntity(reader(members, calls), "crm", id(1), ["crm"], true);
    expect(reply.records).toHaveLength(150);
    expect(reply.recordsLeftOut).toBeUndefined();
    expect(calls.filter(([v]) => v === "node get")).toHaveLength(150);
    const merged = mergeRecords(reply.records!.map((r) => ({ graph: r.graph, key: `${r.graph}/${r.id}`, node: r.node })))!;
    expect(merged.title).toBe(`crm ${id(150).slice(-2)}`);
    expect(merged.fields.find((f) => f.name === "team")).toMatchObject({ value: "only-150" });
  });

  it("counts only the members umx refuses, and fails on anything else", async () => {
    const members = Array.from({ length: 10 }, (_v, i) => ({ graph: "crm", id: id(i + 1) }));
    const base = reader(members);
    const gone = (async (verb: string, args: Record<string, unknown> = {}) => {
      if (verb === "node get" && (args.id === id(3) || args.id === id(9))) throw new UmxError("node_not_found", "gone");
      return base(verb as never, args as never);
    }) as unknown as Parameters<typeof loadEntity>[0];
    const reply = await loadEntity(gone, "crm", id(1), ["crm"], true);
    expect([reply.records!.length, reply.recordsLeftOut]).toEqual([8, 2]);
    const down = (async (verb: string, args: Record<string, unknown> = {}) => {
      if (verb === "node get") throw new UmxError("daemon_not_running", "no daemon");
      return base(verb as never, args as never);
    }) as unknown as Parameters<typeof loadEntity>[0];
    await expect(loadEntity(down, "crm", id(1), ["crm"], true)).rejects.toMatchObject({ code: "daemon_not_running" });
  });

  it("stops between batches once the browser has gone", async () => {
    const ac = new AbortController();
    const calls: [string, unknown][] = [];
    const members = Array.from({ length: 30 }, (_v, i) => ({ graph: "crm", id: id(i + 1) }));
    const base = reader(members, calls);
    const read = (async (verb: string, args: Record<string, unknown> = {}) => {
      if (verb === "node get") ac.abort();
      return base(verb as never, args as never);
    }) as unknown as Parameters<typeof loadEntity>[0];
    await expect(loadEntity(read, "crm", id(1), ["crm"], true, ac.signal)).rejects.toMatchObject({ code: "ui_read_cancelled" });
    expect(calls.filter(([v]) => v === "node get")).toHaveLength(6);
  });
});

describe("replies and offers belong to the open set they were made for (review B3)", () => {
  it("never uses a reply asked over other open graphs, and keeps one across a live tick of the same", () => {
    const asked = `${entityScope("crm/1", ["crm", "vault"], true)}|4`;
    const reply = { ask: asked, records: ["vault's"] };
    expect(replyFor(reply, entityScope("crm/1", ["crm", "vault"], true))).toBe(reply);
    expect(replyFor(reply, entityScope("crm/1", ["crm"], true))).toBeNull();
    expect(replyFor(reply, entityScope("crm/1", ["crm", "vault"], false))).toBeNull();
    expect(replyFor(reply, entityScope("crm/2", ["crm", "vault"], true))).toBeNull();
    expect(scopeOfAsk(`${entityScope("crm/1", ["crm"], false)}|9`)).toBe(entityScope("crm/1", ["crm"], false));
  });

  it("withdraws an offer once the open set changes, and takes its members from what is open now", () => {
    const entities = { list: [{ members: ["crm/1", "ops/1", "vault/1"], graphs: 3, twice: [] }], of: new Map([["crm/1", 0], ["ops/1", 0], ["vault/1", 0]]), proposed: [] };
    const offer = { node: "crm/1", view: "crm,ops,vault" };
    expect(offerMembers(offer, "crm,ops,vault", entities, () => true)).toEqual(["crm/1", "ops/1", "vault/1"]);
    expect(offerMembers(offer, "crm,ops", entities, () => true)).toBeNull();
    expect(offerMembers(offer, "crm,ops,vault", entities, (k) => !k.startsWith("vault/"))).toEqual(["crm/1", "ops/1"]);
    expect(offerMembers({ node: "crm/9", view: "crm,ops,vault" }, "crm,ops,vault", entities, () => true)).toBeNull();
  });
});

describe("values umx flags never leave the server (F188, ws009 M2b)", () => {
  it("shows a member's unflagged keys, hides flagged keys and titles, and redacts each record from its own flags", async () => {
    const SECRET = "ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const members: Fake[] = [
      { graph: "crm", id: id(1), title: SECRET, keys: ["email:jo@example.invalid", `token:${SECRET}`], flags: ["field:title", "key:2"], extra: { title: SECRET, phone: SECRET, secret_suspected: ["field:title", "field:phone"] } },
      { graph: "hr", id: id(2), keys: ["email:jo@example.invalid"], extra: { phone: "555" } },
    ];
    const reply = await loadEntity(reader(members), "crm", id(1), ["crm", "hr"], true);
    expect(reply.members.map((m) => [m.title, m.keys])).toEqual([
      [`person · ${id(1).slice(-6)}`, ["email:jo@example.invalid", "token:(hidden key)"]],
      [`hr ${id(2).slice(-2)}`, ["email:jo@example.invalid"]],
    ]);
    expect(reply.records!.map((r) => [r.node.title, r.node.phone, r.titleHidden ?? false])).toEqual([
      [`person · ${id(1).slice(-6)}`, "(hidden: may be a secret)", true],
      [`hr ${id(2).slice(-2)}`, "555", false],
    ]);
    const text = JSON.stringify(reply);
    expect(text).not.toContain(SECRET);
    expect(text).not.toContain("secret_suspected");
    // Merged mode: the hidden phone is never taken as the same as the other member's.
    const merged = mergeRecords(reply.records!.map((r) => ({ graph: r.graph, key: `${r.graph}/${r.id}`, node: r.node })))!;
    expect(merged.fields.find((f) => f.name === "phone")!.others).toHaveLength(1);
  });
});

describe("a flag the view does not know fails closed in the entity read (review N1)", () => {
  it("hides the member's title and every key", async () => {
    const SECRET = "ghp_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const members: Fake[] = [{ graph: "crm", id: id(1), title: SECRET, keys: ["email:jo@example.invalid", `token:${SECRET}`], flags: ["shape:0"] }];
    const reply = await loadEntity(reader(members), "crm", id(1), ["crm"], false);
    expect(reply.members.map((m) => [m.title, m.keys])).toEqual([[`person · ${id(1).slice(-6)}`, []]]);
    expect(JSON.stringify(reply)).not.toContain(SECRET);
  });
});
