// Launch codes and tab presence (ws009 M1b2): what bin/serve.mjs answers for a `code` line on stdin, and the idle
// clock behind `umx ui --detach`'s idle stop.
import { describe, expect, it } from "vitest";
import {
  CODE_LIFE_MS,
  TAB_LIVE_MS,
  type LaunchState,
  answerCode,
  ENDED,
  endView,
  idleFor,
  pollBegan,
  pollEnded,
  tabLive,
  takeCode,
  whenEnded,
} from "../bin/launch-codes.mjs";

const fresh = (now: number): LaunchState => ({ codes: new Map(), polls: 0, seen: null, started: now, ended: null, onEnd: [] });
const CODE = "k".repeat(43);

describe("launch codes", () => {
  it("arms a code while no tab is connected, and drops it while one is", () => {
    const s = fresh(0);
    expect(tabLive(s, 0)).toBe(false);
    expect(answerCode(s, CODE, 0)).toBe("armed");
    expect(answerCode(s, "short", 0)).toBeNull();
    expect(takeCode(s, CODE, 0)).toBe(true);
    expect(takeCode(s, CODE, 0)).toBe(false);
    pollBegan(s, 1_000);
    const other = "m".repeat(43);
    expect(answerCode(s, other, 50_000)).toBe("tab");
    pollEnded(s, 50_000);
    expect(answerCode(s, other, 50_000 + TAB_LIVE_MS - 1)).toBe("tab");
    expect(answerCode(s, other, 50_000 + TAB_LIVE_MS)).toBe("armed");
  });

  it("forgets a code once it is a minute old", () => {
    const s = fresh(0);
    answerCode(s, CODE, 0);
    expect(takeCode(s, CODE, CODE_LIFE_MS)).toBe(false);
    expect(s.codes.size).toBe(0);
  });

  it("counts idle time from the start, and from the last poll once a tab asked", () => {
    const s = fresh(1_000);
    expect(idleFor(s, 5_000)).toBe(4_000);
    pollBegan(s, 6_000);
    expect(idleFor(s, 60_000)).toBe(0);
    pollEnded(s, 60_000);
    expect(idleFor(s, 90_000)).toBe(30_000);
  });
});

describe("why the view ends (ui-expert N1)", () => {
  it("answers every waiting and later poll with the reason umx sent, and only for its own words", async () => {
    const s = fresh(0);
    const waiting = whenEnded(s);
    expect(endView(s, "rm -rf")).toBe(false);
    expect(s.ended).toBeNull();
    expect(endView(s, "restarted")).toBe(true);
    expect(await waiting).toBe("restarted");
    expect(await whenEnded(s)).toBe("restarted");
    expect(endView(s, "unreachable")).toBe(false);
    expect(ENDED[s.ended!]).toBe("The graph view ended (umx daemon restarted). Run umx ui --detach to start it again.");
  });
});

