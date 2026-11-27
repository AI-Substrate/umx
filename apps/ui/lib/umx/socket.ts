// The view's own requests on the daemon socket (ws009 S6b, plan 114): the long poll UiWait (with the change feed, S5b
// plan 127), UiSelect, which publishes the person's selection (S7, plan 117, lib/publish.ts), and the person's
// "Clear agent marks", the `umx ui clear` command (lib/publish.ts). Ops only the view sends have no CLI verb (Op::VIEW_ONLY);
// every other read goes through the CLI (lib/umx/index.ts). The wire is umx's own client's: one JSON request, the
// write half shut, one envelope back (crates/umx-cli/src/lib.rs call_with). This file and lib/umx/index.ts are the
// only modules that open a socket or run a process (architect R2); this one may read file metadata (lstat) to check
// the run folder, never file contents.
import { lstat } from "node:fs/promises";
import { connect } from "node:net";
import { homedir, userInfo } from "node:os";
import type { DaemonChange, Ids } from "@/lib/live";
import { MAX_CHANGE_IDS } from "@/lib/live";
import { VIEW_ROLE } from "@/lib/agent";
import { MAX_OPEN } from "@/lib/graph/keys";
import { alias, nodeId, UMX_PROTOCOL, type UmxIssue, UmxError } from "@/lib/umx";

/** Largest reply the UI reads from the socket, in bytes: the UI's own cap, not umx-protocol's (MAX_FRAME caps requests, MAX_REPLY replies). Pairs come through the CLI's stdout instead. */
export const MAX_FRAME = 16 << 20;
/** umx-core ui: the longest wait, the most graphs one wait names, the longest note. */
export const MAX_WAIT_MS = 25_000;
export const MAX_WAIT_GRAPHS = 64;
export const MAX_NOTE_CHARS = 200;
/** umx-core ui MAX_SELECTION: the most nodes one selection holds. */
export const MAX_SELECTION = 1_000;
/** The longest Unix socket path macOS takes (umx daemon start refuses past it too). */
const MAX_SOCKET_PATH = 104;
const EPOCH = /^[0-9a-f]{16}$/;
/** How long past the wait itself a reply may take before the view gives up on it. */
const GRACE_MS = 10_000;
/** Who the view is on the wire. The daemon only warns that it is not its own build (daemon_version_mismatch). */
const BUILD = { version: "0.0.0", commit: "umx-ui", built: "apps/ui" };


/** Who sent an event: umx's sanitized actor when it carries one (P2), else nothing. A claim, never verified. */
export type AgentActor = { seat?: string; role?: string; harness?: string; user?: string };

export type UiCommand =
  /** `centre` (PROTOCOL 110, PRD 005, `umx ui focus --centre`): the view also centres in rings of hops on the node.
   * `select` (PROTOCOL 111, `umx ui focus --select`): the view also selects the node, opening its panel. */
  | { kind: "focus"; node: string; hops: number; centre?: boolean; select?: boolean; note?: string }
  | { kind: "highlight"; nodes: string[]; add: boolean; note?: string }
  | { kind: "filter"; show: string[]; hide: string[]; note?: string }
  | { kind: "fit"; nodes: string[]; note?: string }
  | { kind: "clear"; note?: string }
  /** ws009 M1b1: open these graphs (aliases the daemon accepted, never a secured one), added, or `only` in place.
   * `left_out`: how many graphs the agent named that the daemon refused, counted and never named. `raise` (M1b2,
   * `umx ui open --raise`): the agent asks the person to look, so the tab marks its title and asks for focus. */
  | { kind: "open"; graphs: string[]; only: boolean; left_out: number; raise?: boolean; note?: string };

export type AgentEvent = { seq: number; at_ms: number; graph: string; by: AgentActor | null; command: UiCommand };

/** UiWait's answer (crates/umx-daemon/src/ui_channel.rs waited_json), with the change feed (S5a, plan 120). */
export type Waited = { epoch: string; seq: number; reset: boolean; events: AgentEvent[]; changes: DaemonChange[] };

/** `revs`: the revision the view has for each named graph it follows (umx docs get ui, "The change feed"). */
export type WaitArgs = { epoch: string | null; since: number; timeoutMs: number; graphs: string[]; revs?: Record<string, number> };

/** What lstat says of a path, or null when nothing is there. */
export type Stat = { dir: boolean; socket: boolean; link: boolean; uid: number; mode: number };

export type SocketDeps = {
  env: () => Record<string, string | undefined>;
  uid: () => number;
  stat: (path: string) => Promise<Stat | null>;
  /** Sends one request and reads the whole reply. */
  exchange: (path: string, body: string, timeoutMs: number, signal?: AbortSignal) => Promise<string>;
};

/** `$UMX_HOME/run/umx.sock`, else `~/.umx/run/umx.sock`, as umx resolves it (umx-compose Config::at). */
export function socketPath(env: Record<string, string | undefined>): { run: string; socket: string } {
  const home = env.UMX_HOME && env.UMX_HOME !== "" ? env.UMX_HOME : `${homedir()}/.umx`;
  const run = `${home.replace(/\/+$/, "")}/run`;
  return { run, socket: `${run}/umx.sock` };
}

const refused = (why: string) =>
  new UmxError("ui_run_dir_refused", `The view will not use the umx socket: ${why}.`, "umx doctor", 500);

/**
 * The checks umx daemon start makes before it binds (socket_unavailable): the run folder is a plain folder, not a
 * link, owned by this user, mode 0700 (architect, S6b); the socket is a socket, also this user's. A folder others can
 * write could hold someone else's socket, which would then hear the view's requests and answer them.
 */
export async function checkRun(deps: SocketDeps): Promise<string> {
  const { run, socket } = socketPath(deps.env());
  if (Buffer.byteLength(socket) > MAX_SOCKET_PATH) throw refused(`its path is over ${MAX_SOCKET_PATH} bytes`);
  const uid = deps.uid();
  const dir = await deps.stat(run);
  if (dir === null) throw new UmxError("daemon_not_running", "The umx daemon is not running.", "umx daemon start", 503);
  if (dir.link || !dir.dir) throw refused("its run folder is not a plain folder");
  if (dir.uid !== uid) throw refused("its run folder belongs to another user");
  if ((dir.mode & 0o777) !== 0o700) throw refused("its run folder is not mode 0700 (chmod 700 it)");
  const sock = await deps.stat(socket);
  if (sock === null) throw new UmxError("daemon_not_running", "The umx daemon is not running.", "umx daemon start", 503);
  if (sock.link || !sock.socket) throw refused("something that is not a socket is at its path");
  if (sock.uid !== uid) throw refused("the socket belongs to another user");
  return socket;
}

const text = (v: unknown): string | null => (typeof v === "string" ? v : null);
const whole = (v: unknown): number | null => (typeof v === "number" && Number.isSafeInteger(v) && v >= 0 ? v : null);
const ids = (v: unknown): string[] | null => (Array.isArray(v) && v.every((n) => typeof n === "string") ? (v as string[]) : null);
const bad = (what: string) => new UmxError("ui_umx_output", `umx answered a malformed ${what}.`);
/** An id or alias in the daemon's reply that fails the view's own check is the daemon's fault, not the browser's. */
function checked(check: (v: string | undefined) => string, v: unknown, what: string): string {
  try {
    return check(text(v) ?? undefined);
  } catch {
    throw bad(what);
  }
}

function note(v: unknown): string | undefined {
  if (v === undefined || v === null) return undefined;
  const t = text(v);
  if (t === null || [...t].length > MAX_NOTE_CHARS) throw bad("note");
  return t;
}

/** One event's command as the daemon writes it (umx-protocol UiCmd, node ids in place of what the agent typed). */
function command(v: unknown): UiCommand {
  if (typeof v !== "object" || v === null) throw bad("ui command");
  const c = v as Record<string, unknown>;
  const n = note(c.note);
  const withNote = <T extends object>(o: T) => (n === undefined ? o : { ...o, note: n });
  switch (c.kind) {
    case "focus": {
      const hops = whole(c.hops);
      const centre: unknown = c.centre ?? false;
      const select: unknown = c.select ?? false;
      if (hops === null || hops < 1 || hops > 3 || typeof centre !== "boolean" || typeof select !== "boolean") throw bad("focus");
      return withNote({ kind: "focus", node: checked(nodeId, c.node, "node id"), hops, ...(centre ? { centre: true } : {}), ...(select ? { select: true } : {}) });
    }
    case "highlight": {
      const nodes = ids(c.nodes);
      if (nodes === null || typeof c.add !== "boolean") throw bad("highlight");
      return withNote({ kind: "highlight", nodes: nodes.map((x) => checked(nodeId, x, "node id")), add: c.add });
    }
    case "filter": {
      const show = ids(c.show);
      const hide = ids(c.hide);
      if (show === null || hide === null) throw bad("filter");
      return withNote({ kind: "filter", show, hide });
    }
    case "fit": {
      const nodes = ids(c.nodes);
      if (nodes === null) throw bad("fit");
      return withNote({ kind: "fit", nodes: nodes.map((x) => checked(nodeId, x, "node id")) });
    }
    case "clear":
      return withNote({ kind: "clear" });
    case "open": {
      const graphs = ids(c.graphs);
      const leftOut: unknown = c.left_out ?? 0;
      if (graphs === null || graphs.length === 0 || graphs.length > MAX_OPEN || typeof c.only !== "boolean") throw bad("open");
      if (typeof leftOut !== "number" || !Number.isInteger(leftOut) || leftOut < 0 || leftOut > MAX_OPEN) throw bad("open");
      const raise: unknown = c.raise ?? false;
      if (typeof raise !== "boolean") throw bad("open");
      return withNote({ kind: "open", graphs: graphs.map((g) => checked(alias, g, "graph alias")), only: c.only, left_out: leftOut, raise });
    }
    default:
      throw bad("ui command");
  }
}

function actor(v: unknown): AgentActor | null {
  if (typeof v !== "object" || v === null) return null;
  const a = v as Record<string, unknown>;
  const out: AgentActor = {};
  for (const k of ["seat", "role", "harness", "user"] as const) {
    const t = text(a[k]);
    if (t !== null) out[k] = t;
  }
  return out;
}

function idList(v: unknown): Ids {
  if (typeof v !== "object" || v === null) throw bad("change");
  const o = v as Record<string, unknown>;
  const list = (x: unknown) => {
    const l = ids(x);
    if (l === null || l.length > MAX_CHANGE_IDS) throw bad("change");
    return l.map((id) => checked(nodeId, id, "id"));
  };
  return { added: list(o.added), edited: list(o.edited), removed: list(o.removed) };
}

/** One graph's change: ids and revisions only (security T2), for a graph the wait named. */
function change(v: unknown, named: ReadonlySet<string> | null): DaemonChange {
  if (typeof v !== "object" || v === null) throw bad("change");
  const c = v as Record<string, unknown>;
  const graph = checked(alias, c.graph, "graph alias");
  if (named !== null && !named.has(graph)) throw bad("change");
  if (c.overflow === true) {
    const rev = whole(c.rev);
    if (rev === null) throw bad("change");
    return { graph, overflow: true, rev };
  }
  const from = whole(c.from_rev);
  const to = whole(c.to_rev);
  if (from === null || to === null || to <= from) throw bad("change");
  return { graph, overflow: false, from_rev: from, to_rev: to, nodes: idList(c.nodes), edges: idList(c.edges) };
}

/** Reads UiWait's data; anything unexpected is refused, never half read. `named`: the graphs the wait named. */
export function readWaited(data: unknown, named: readonly string[] | null = null): Waited {
  if (typeof data !== "object" || data === null) throw bad("wait reply");
  const d = data as Record<string, unknown>;
  const epoch = text(d.epoch);
  const seq = whole(d.seq);
  if (epoch === null || !EPOCH.test(epoch) || seq === null || typeof d.reset !== "boolean" || !Array.isArray(d.events)) {
    throw bad("wait reply");
  }
  const events = d.events.map((raw): AgentEvent => {
    const e = (raw ?? {}) as Record<string, unknown>;
    const s = whole(e.seq);
    const at = whole(e.at_ms);
    if (s === null || at === null) throw bad("event");
    return { seq: s, at_ms: at, graph: checked(alias, e.graph, "graph alias"), by: actor(e.by), command: command(e.command) };
  });
  if (d.changes !== undefined && !Array.isArray(d.changes)) throw bad("wait reply");
  const only = named === null ? null : new Set(named);
  const changes = ((d.changes as unknown[] | undefined) ?? []).map((c) => change(c, only));
  return { epoch, seq, reset: d.reset, events, changes };
}

type Reply = { umx?: unknown; command?: unknown; status?: unknown; data?: unknown; errors?: UmxIssue[] };

function readReply(raw: string, expected: string): unknown {
  let env: Reply;
  try {
    env = JSON.parse(raw) as Reply;
  } catch {
    throw new UmxError("ui_umx_output", "The umx daemon did not answer with JSON.");
  }
  const first = Array.isArray(env.errors) ? env.errors[0] : undefined;
  if (first || env.status === "error") {
    throw new UmxError(String(first?.code ?? "umx_error"), String(first?.message ?? "umx refused the request."), first?.next ?? null);
  }
  if (env.umx !== UMX_PROTOCOL || env.command !== expected) {
    throw new UmxError("ui_umx_output", `The umx daemon answered "${String(env.command).slice(0, 40)}" to ${expected}.`);
  }
  return env.data;
}

function request(command: string, op: Record<string, unknown>, graph?: string, actor?: Record<string, string>): string {
  return JSON.stringify({
    umx: UMX_PROTOCOL, command, args: [], client: BUILD, op, ...(graph === undefined ? {} : { graph }), ...(actor ? { actor } : {}),
  });
}

/** The OS user the view runs as, as umx's client names its writer (the cli tier needs one); "unknown" otherwise. */
function viewUser(): string {
  try {
    const name = userInfo().username;
    return /^[A-Za-z0-9._:@-]{1,128}$/.test(name) ? name : "unknown";
  } catch {
    return "unknown";
  }
}

export function createSocket(deps: SocketDeps) {
  return {
    /** The events after `since` for `graphs` (the graphs the view has open), or none after `timeoutMs`. */
    async wait(args: WaitArgs, signal?: AbortSignal): Promise<Waited> {
      const graphs = args.graphs.map((g) => alias(g));
      if (graphs.length === 0 || graphs.length > MAX_WAIT_GRAPHS) throw new UmxError("ui_bad_wait", "A wait names 1 to 64 graphs.", null, 400);
      if (!Number.isSafeInteger(args.timeoutMs) || args.timeoutMs < 0 || args.timeoutMs > MAX_WAIT_MS) {
        throw new UmxError("ui_bad_wait", "A wait lasts at most 25000 ms.", null, 400);
      }
      if (!Number.isSafeInteger(args.since) || args.since < 0) throw new UmxError("ui_bad_wait", "since is a whole number.", null, 400);
      if (args.epoch !== null && !EPOCH.test(args.epoch)) throw new UmxError("ui_bad_wait", "epoch is 16 hex digits.", null, 400);
      const revs: Record<string, number> = {};
      for (const [g, rev] of Object.entries(args.revs ?? {})) {
        if (!graphs.includes(g) || !Number.isSafeInteger(rev) || rev < 0) {
          throw new UmxError("ui_bad_wait", "revs holds a whole number for a named graph.", null, 400);
        }
        revs[g] = rev;
      }
      const socket = await checkRun(deps);
      const op = { op: "ui_wait", epoch: args.epoch, since: args.since, timeout_ms: args.timeoutMs, graphs, revs };
      const raw = await deps.exchange(socket, request("ui wait", op), args.timeoutMs + GRACE_MS, signal);
      return readWaited(readReply(raw, "ui wait"), graphs);
    },
    /** Publishes what the person selected in `graph`, replacing the last (no nodes clears it). */
    async select(graph: string, nodes: string[]): Promise<void> {
      if (nodes.length > MAX_SELECTION) throw new UmxError("ui_bad_selection", "A selection is at most 1000 nodes.", null, 400);
      const socket = await checkRun(deps);
      const op = { op: "ui_select", nodes: nodes.map((n) => nodeId(n)) };
      readReply(await deps.exchange(socket, request("ui select", op, alias(graph)), GRACE_MS), "ui select");
    },
    /** `umx ui clear` for `graph`, sent by the person: takes off what agents set there, in every view, for good. */
    async clear(graph: string): Promise<void> {
      const socket = await checkRun(deps);
      const op = { op: "ui_command", command: { kind: "clear" } };
      const actor = { tier: "cli", role: VIEW_ROLE, user: viewUser() };
      readReply(await deps.exchange(socket, request("ui clear", op, alias(graph), actor), GRACE_MS), "ui clear");
    },
  };
}

const exchangeUnix: SocketDeps["exchange"] = (path, body, timeoutMs, signal) =>
  new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    let settled = false;
    const sock = connect(path);
    const finish = (error: unknown, value?: string) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      sock.destroy();
      if (error) reject(error);
      else resolve(value ?? "");
    };
    const onAbort = () => finish(new UmxError("ui_wait_aborted", "The wait was stopped.", null, 499));
    const timer = setTimeout(() => finish(new UmxError("ui_daemon_timeout", "The umx daemon did not answer in time.", "umx daemon status", 504)), timeoutMs);
    if (signal?.aborted) return onAbort();
    signal?.addEventListener("abort", onAbort, { once: true });
    sock.on("connect", () => sock.end(body));
    sock.on("data", (chunk: Buffer) => {
      size += chunk.length;
      if (size > MAX_FRAME) return finish(new UmxError("ui_umx_output", "The umx daemon's reply is too big."));
      chunks.push(chunk);
    });
    sock.on("end", () => finish(null, Buffer.concat(chunks).toString("utf8")));
    sock.on("error", (e: NodeJS.ErrnoException) => {
      const down = e.code === "ENOENT" || e.code === "ECONNREFUSED";
      // The error's text holds the socket path: the server's terminal only (as lib/umx keeps argv out of replies).
      if (!down) console.error(`umx socket: ${e.code ?? "error"}`);
      finish(
        down
          ? new UmxError("daemon_not_running", "The umx daemon is not running.", "umx daemon start", 503)
          : new UmxError("ui_socket", "The view could not talk to the umx daemon; see its terminal.", "umx daemon status", 502),
      );
    });
  });

async function statPath(path: string): Promise<Stat | null> {
  try {
    const s = await lstat(path);
    return { dir: s.isDirectory(), socket: s.isSocket(), link: s.isSymbolicLink(), uid: s.uid, mode: s.mode };
  } catch {
    return null;
  }
}

let shared: ReturnType<typeof createSocket> | null = null;

/** The process-wide socket client, wired to the real daemon. */
export function umxSocket(): ReturnType<typeof createSocket> {
  shared ??= createSocket({
    env: () => process.env,
    uid: () => process.getuid?.() ?? -1,
    stat: statPath,
    exchange: exchangeUnix,
  });
  return shared;
}
