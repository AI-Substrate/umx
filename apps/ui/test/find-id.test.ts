// Find by id (lib/find-id): the Find box takes a full id, a unique prefix or a umx: address and names one canvas node.
import { describe, expect, it } from "vitest";
import { findById, idQuery } from "@/lib/find-id";

const A = "01KQ7Z3M4N5P6Q7R8S9T0VWXYZ";
const B = "01KQ7Z3M4N5P6Q7R8S9T0VWXY0";
const C = "01KR0000000000000000000000";
const keys = [`demo/${A}`, `demo/${B}`, `demo/${C}`, `other/${C}`];

describe("finding a node by id", () => {
  it("takes a full id, a unique prefix in any case, or an address, and refuses words", () => {
    expect(findById(A, keys)).toEqual({ kind: "one", key: `demo/${A}` });
    expect(findById(A.slice(0, 25).toLowerCase(), keys)).toEqual({ kind: "many", count: 2 });
    expect(findById(`  ${A.slice(0, 26)}  `, keys)).toEqual({ kind: "one", key: `demo/${A}` });
    expect(findById(`umx:other/${C}`, keys)).toEqual({ kind: "one", key: `other/${C}` });
    expect(findById("umx:demo/01kr", keys)).toEqual({ kind: "one", key: `demo/${C}` });
    expect(findById("01KQ7Z", keys)).toEqual({ kind: "many", count: 2 });
    expect(findById("01KS", keys)).toEqual({ kind: "none" });
    expect(findById(`umx:gone/${A}`, keys)).toEqual({ kind: "none" });
    for (const word of ["data", "risk", "01K", "budget 2026", "umx:demo", `${A}X`]) expect(findById(word, keys), word).toBeNull();
    expect(idQuery("umx:demo/01kr")).toEqual({ graph: "demo", id: "01KR" });
  });

  it("names both copies when one full id is in two open graphs, and the exact id over longer ones", () => {
    expect(findById(C, keys)).toEqual({ kind: "many", count: 2 });
    expect(findById(C, [`demo/${C}`, `demo/${C}1`])).toEqual({ kind: "one", key: `demo/${C}` });
  });
});
