// The view's socket client (lib/umx/socket.ts): the run-folder checks umx daemon start makes, the exact request
// UiWait and UiSelect send, and a reply read strictly (crates/umx-daemon/src/ui_channel.rs waited_json).
import { describe, expect, it, vi } from "vitest";
import { checkRun, createSocket, type SocketDeps, type Stat, type WaitArgs } from "@/lib/umx/socket";
import { UMX_PROTOCOL, UmxError } from "@/lib/umx";

const UID = 501;
const DIR: Stat = { dir: true, socket: false, link: false, uid: UID, mode: 0o40700 };
const SOCK: Stat = { dir: false, socket: true, link: false, uid: UID, mode: 0o140600 };
const N = "01NODEAAAAAAAAAAAAAAAAAAAA";
const EPOCH = "00a1b2c3d4e5f607";

function deps(over: { dir?: Stat | null; sock?: Stat | null; reply?: unknown; home?: string } = {}) {
  const exchange = vi.fn<SocketDeps["exchange"]>(async () => JSON.stringify(over.reply ?? waited()));
  const d: SocketDeps = {
    env: () => ({ UMX_HOME: over.home ?? "/h/umx" }),
    uid: () => UID,
    stat: async (p) => (p.endsWith("/run") ? (over.dir === undefined ? DIR : over.dir) : over.sock === undefined ? SOCK : over.sock),
    exchange,
  };
  return { d, exchange };
}

function waited(data: object = { epoch: EPOCH, seq: 3, reset: false, events: [] }, more: object = {}) {
  return { umx: UMX_PROTOCOL, command: "ui wait", status: "ok", data, warnings: [], errors: [], untrusted: ["data"], ...more };
}

async function refusal(p: Promise<unknown>): Promise<UmxError> {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(UmxError);
    return e as UmxError;
  }
  throw new Error("expected a refusal");
}

describe("the run folder", () => {
  it("uses $UMX_HOME/run/umx.sock when the folder is ours and 0700 and the socket is ours", async () => {
    expect(await checkRun(deps().d)).toBe("/h/umx/run/umx.sock");
  });

  it("refuses a folder that is a link, not a folder, another user's or not 0700, and a socket that is not one or not ours", async () => {
    for (const dir of [{ ...DIR, link: true }, { ...DIR, dir: false }, { ...DIR, uid: 0 }, { ...DIR, mode: 0o40750 }, { ...DIR, mode: 0o40777 }, { ...DIR, mode: 0o40500 }]) {
      expect((await refusal(checkRun(deps({ dir }).d))).code).toBe("ui_run_dir_refused");
    }
    for (const sock of [{ ...SOCK, socket: false }, { ...SOCK, link: true }, { ...SOCK, uid: 0 }]) {
      expect((await refusal(checkRun(deps({ sock }).d))).code).toBe("ui_run_dir_refused");
    }
    expect((await refusal(checkRun(deps({ home: `/${"x".repeat(100)}` }).d))).code).toBe("ui_run_dir_refused");
  });

  it("says the daemon is not running when the folder or socket is missing, and never connects", async () => {
    for (const over of [{ dir: null }, { sock: null }]) {
      const { d, exchange } = deps(over);
      expect((await refusal(createSocket(d).wait({ epoch: null, since: 0, timeoutMs: 1000, graphs: ["demo"] }))).code).toBe("daemon_not_running");
      expect(exchange).not.toHaveBeenCalled();
    }
    const { d, exchange } = deps({ dir: { ...DIR, mode: 0o40777 } });
    await refusal(createSocket(d).wait({ epoch: null, since: 0, timeoutMs: 1000, graphs: ["demo"] }));
    expect(exchange).not.toHaveBeenCalled();
  });
});

describe("UiWait", () => {
  it("sends the view-only op with this view's protocol, and reads events strictly", async () => {
    const event = { seq: 3, at_ms: 1_700_000_000_000, graph: "demo", command: { kind: "focus", node: N, hops: 2, note: "look here" } };
    const { d, exchange } = deps({ reply: waited({ epoch: EPOCH, seq: 3, reset: true, events: [event] }) });
    const got = await createSocket(d).wait({ epoch: null, since: 0, timeoutMs: 25_000, graphs: ["demo", "vault"], revs: { demo: 4 } });
    const [path, body, timeout] = exchange.mock.calls[0];
    expect(path).toBe("/h/umx/run/umx.sock");
    expect(JSON.parse(body)).toEqual({
      umx: UMX_PROTOCOL, command: "ui wait", args: [], client: { version: "0.0.0", commit: "umx-ui", built: "apps/ui" },
      op: { op: "ui_wait", epoch: null, since: 0, timeout_ms: 25_000, graphs: ["demo", "vault"], revs: { demo: 4 } },
    });
    expect(timeout).toBeGreaterThan(25_000);
    expect(got).toEqual({ epoch: EPOCH, seq: 3, reset: true, events: [{ ...event, by: null }], changes: [] });
  });

  it("reads an agent's focus --centre (PROTOCOL 110, PRD 005 centre-11)", async () => {
    const event = (centre?: boolean) => ({ seq: 3, at_ms: 1, graph: "demo", command: { kind: "focus", node: N, hops: 1, ...(centre === undefined ? {} : { centre }) } });
    for (const [sent, read] of [[true, { centre: true }], [false, {}], [undefined, {}]] as const) {
      const { d } = deps({ reply: waited({ epoch: EPOCH, seq: 3, reset: false, events: [event(sent)] }) });
      const got = await createSocket(d).wait({ epoch: null, since: 0, timeoutMs: 1, graphs: ["demo"] });
      expect(got.events[0].command).toEqual({ kind: "focus", node: N, hops: 1, ...read });
    }
  });

  it("reads who sent an event from umx's serialised actor, keeping only what it shows (plan 129)", async () => {
    // The daemon's `by` as umx-daemon's s6a test a_ui_command_event_carries_the_checked_sender_and_never_a_raw_session
    // emits it: every ActorInput field, the session hashed.
    const by = {
      tier: "pij", seat: "pij-calm-otter", role: "coder", model: "openai/gpt-5.6", effort: null,
      harness: "copilot", harness_session: "h-0123456789abcdef", user: "writer",
    };
    const event = { seq: 4, at_ms: 1_700_000_000_000, graph: "demo", by, command: { kind: "clear" } };
    const { d } = deps({ reply: waited({ epoch: EPOCH, seq: 4, reset: false, events: [event] }) });
    const got = await createSocket(d).wait({ epoch: EPOCH, since: 3, timeoutMs: 1000, graphs: ["demo"] });
    expect(got.events[0].by).toEqual({ seat: "pij-calm-otter", role: "coder", harness: "copilot", user: "writer" });
  });

  it("reads the change feed: ids and revisions per named graph, or an overflow (S5b)", async () => {
    const E = "01EDGEAAAAAAAAAAAAAAAAAAAA";
    const changes = [
      { graph: "demo", from_rev: 4, to_rev: 6, nodes: { added: [N], edited: [], removed: [] }, edges: { added: [], edited: [], removed: [E] } },
      { graph: "vault", rev: 9, overflow: true },
    ];
    const { d } = deps({ reply: waited({ epoch: EPOCH, seq: 3, reset: false, events: [], changes }) });
    const got = await createSocket(d).wait({ epoch: EPOCH, since: 3, timeoutMs: 1, graphs: ["demo", "vault"], revs: { demo: 4, vault: 0 } });
    expect(got.changes).toEqual([
      { graph: "demo", overflow: false, from_rev: 4, to_rev: 6, nodes: { added: [N], edited: [], removed: [] }, edges: { added: [], edited: [], removed: [E] } },
      { graph: "vault", overflow: true, rev: 9 },
    ]);
  });

  it("refuses a change for a graph the wait did not name, a malformed id, a list over the cap or revisions that go back", async () => {
    const ids = (added: unknown[]) => ({ added, edited: [], removed: [] });
    const one = (c: object) => waited({ epoch: EPOCH, seq: 3, reset: false, events: [], changes: [c] });
    const bad = [
      one({ graph: "other", rev: 1, overflow: true }),
      one({ graph: "demo", from_rev: 1, to_rev: 2, nodes: ids(["x"]), edges: ids([]) }),
      one({ graph: "demo", from_rev: 1, to_rev: 2, nodes: ids(Array.from({ length: 501 }, () => N)), edges: ids([]) }),
      one({ graph: "demo", from_rev: 2, to_rev: 2, nodes: ids([]), edges: ids([]) }),
      one({ graph: "demo", rev: -1, overflow: true }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [], changes: {} }),
    ];
    for (const reply of bad) {
      expect((await refusal(createSocket(deps({ reply }).d).wait({ epoch: null, since: 0, timeoutMs: 1, graphs: ["demo"] }))).code).toBe("ui_umx_output");
    }
  });

  it("refuses bad arguments before anything runs", async () => {
    const { d, exchange } = deps();
    const s = createSocket(d);
    for (const args of <WaitArgs[]>[
      { epoch: null, since: 0, timeoutMs: 25_001, graphs: ["demo"] },
      { epoch: null, since: -1, timeoutMs: 1, graphs: ["demo"] },
      { epoch: "nothex", since: 0, timeoutMs: 1, graphs: ["demo"] },
      { epoch: null, since: 0, timeoutMs: 1, graphs: [] },
      { epoch: null, since: 0, timeoutMs: 1, graphs: ["--graph"] },
      { epoch: null, since: 0, timeoutMs: 1, graphs: ["demo"], revs: { other: 1 } },
      { epoch: null, since: 0, timeoutMs: 1, graphs: ["demo"], revs: { demo: -1 } },
      { epoch: null, since: 0, timeoutMs: 1, graphs: ["demo"], revs: { demo: 1.5 } },
    ]) {
      await refusal(s.wait(args));
    }
    expect(exchange).not.toHaveBeenCalled();
  });

  it("passes the daemon's refusal on as given (protocol_mismatch, ui_waiters_busy)", async () => {
    for (const code of ["protocol_mismatch", "ui_waiters_busy"]) {
      const reply = waited({}, { umx: 36, status: "error", data: null, errors: [{ code, message: "no", next: "umx daemon status" }] });
      const e = await refusal(createSocket(deps({ reply }).d).wait({ epoch: null, since: 0, timeoutMs: 1, graphs: ["demo"] }));
      expect([e.code, e.next]).toEqual([code, "umx daemon status"]);
    }
  });

  it("refuses a reply from another protocol, for another command, or with a malformed event", async () => {
    const bad = [
      waited(undefined, { umx: UMX_PROTOCOL - 1 }),
      waited(undefined, { command: "help" }),
      waited({ epoch: 7, seq: 3, reset: false, events: [] }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "Demo", command: { kind: "clear" } }] }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "focus", node: "x", hops: 1 } }] }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "focus", node: N, hops: 4 } }] }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "clear", note: "x".repeat(201) } }] }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "open_graph" } }] }),
      // ws009 M1b1: an open names 1 to 12 aliases, each a graph name, and says whether it is --only.
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "open", graphs: [], only: false } }] }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "open", graphs: ["Demo"], only: false } }] }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "open", graphs: Array.from({ length: 13 }, (_, i) => `g${i}`), only: false } }] }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "open", graphs: ["demo"], only: "yes" } }] }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "open", graphs: ["demo"], only: false, left_out: 13 } }] }),
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "open", graphs: ["demo"], only: false, left_out: "1" } }] }),
      // ws009 M1b2: raise is a boolean when there.
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "open", graphs: ["demo"], only: false, raise: "yes" } }] }),
      // PRD 005: centre is a boolean when there.
      waited({ epoch: EPOCH, seq: 3, reset: false, events: [{ seq: 1, at_ms: 1, graph: "demo", command: { kind: "focus", node: N, hops: 1, centre: "yes" } }] }),
    ];
    for (const reply of bad) {
      expect((await refusal(createSocket(deps({ reply }).d).wait({ epoch: null, since: 0, timeoutMs: 1, graphs: ["demo"] }))).code).toBe("ui_umx_output");
    }
  });
});

describe("an agent's open (ws009 M1b1)", () => {
  it("reads its graphs and only switch", async () => {
    const command = { kind: "open", graphs: ["alpha", "beta"], only: true, left_out: 1, raise: true, note: "look" };
    const reply = waited({ epoch: EPOCH, seq: 1, reset: false, events: [{ seq: 1, at_ms: 1, graph: "alpha", command }] });
    const got = await createSocket(deps({ reply }).d).wait({ epoch: null, since: 0, timeoutMs: 1, graphs: ["alpha"] });
    expect(got.events[0].command).toEqual(command);
    // An event with no count and no raise (the daemon leaves both out when 0 and false): none left out, no raise.
    const older = { kind: command.kind, graphs: command.graphs, only: command.only, note: command.note };
    const plain = waited({ epoch: EPOCH, seq: 1, reset: false, events: [{ seq: 1, at_ms: 1, graph: "alpha", command: older }] });
    expect((await createSocket(deps({ reply: plain }).d).wait({ epoch: null, since: 0, timeoutMs: 1, graphs: ["alpha"] })).events[0].command).toMatchObject({ left_out: 0, raise: false });
  });
});

describe("UiSelect (S7)", () => {
  it("names the graph and sends ids only", async () => {
    const { d, exchange } = deps({ reply: { ...waited({ graph: "demo", selected: 1 }), command: "ui select" } });
    await createSocket(d).select("demo", [N]);
    expect(JSON.parse(exchange.mock.calls[0][1])).toMatchObject({ command: "ui select", graph: "demo", op: { op: "ui_select", nodes: [N] } });
    await refusal(createSocket(d).select("demo", ["--graph"]));
    expect((await refusal(createSocket(d).select("demo", Array.from({ length: 1001 }, () => N)))).code).toBe("ui_bad_selection");
    expect(exchange).toHaveBeenCalledTimes(1);
  });
});

describe("the person's Clear agent marks", () => {
  it("sends umx ui clear for the graph, claimed as the view, and nothing for a bad alias", async () => {
    const { d, exchange } = deps({ reply: { ...waited({ seq: 9 }), command: "ui clear" } });
    await createSocket(d).clear("demo");
    const sent = JSON.parse(exchange.mock.calls[0][1]);
    expect(sent).toMatchObject({ command: "ui clear", graph: "demo", op: { op: "ui_command", command: { kind: "clear" } } });
    expect(sent.actor).toMatchObject({ tier: "cli", role: "graph_view" });
    expect(sent.actor.user).toMatch(/^[A-Za-z0-9._:@-]+$/);
    await refusal(createSocket(d).clear("--all"));
    expect(exchange).toHaveBeenCalledTimes(1);
  });
});

describe("an agent's focus --select (PROTOCOL 111)", () => {
  it("reads select only as a boolean and keeps it on the event", async () => {
    const focus = (extra: object) => ({ seq: 1, at_ms: 1, graph: "demo", by: null, command: { kind: "focus", node: N, hops: 1, ...extra } });
    const { d } = deps({ reply: waited({ epoch: EPOCH, seq: 1, reset: false, events: [focus({ select: true })] }) });
    const got = await createSocket(d).wait({ epoch: null, since: 0, timeoutMs: 0, graphs: ["demo"] });
    expect(got.events[0].command).toEqual({ kind: "focus", node: N, hops: 1, select: true });
    const plain = deps({ reply: waited({ epoch: EPOCH, seq: 1, reset: false, events: [focus({ select: false })] }) });
    expect((await createSocket(plain.d).wait({ epoch: null, since: 0, timeoutMs: 0, graphs: ["demo"] })).events[0].command).toEqual({ kind: "focus", node: N, hops: 1 });
    const bad = deps({ reply: waited({ epoch: EPOCH, seq: 1, reset: false, events: [focus({ select: "yes" })] }) });
    await refusal(createSocket(bad.d).wait({ epoch: null, since: 0, timeoutMs: 0, graphs: ["demo"] }));
  });
});
