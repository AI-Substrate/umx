"use client";
// The node panel's Spines block: every spine the node's graph declares, how this node sits on each (its band's anchor
// and how umx placed it), and a toggle per spine that is the same pick as the sidebar's Spine chips.
import { Loader2 } from "lucide-react";
import { SpineIcon } from "@/components/spine-picker";
import type { NodeOnSpine } from "@/lib/graph/spine-layout";
import { cn } from "@/lib/utils";

export type NodeSpinesView = {
  /** The node's spines, null while its graph's spines are being listed, "failed" when they could not be. */
  rows: NodeOnSpine[] | null | "failed";
  /** An anchor's canvas key and title, from its umx id. */
  anchor: (id: string) => { key: string; title: string };
  onToggle: (name: string) => void;
  onPick: (key: string) => void;
};

const BY: Record<string, string> = { field: "by its own field", link: "by a link", evidence: "by its evidence", rule: "by the spine's rule" };
const SIDE = { before: "before the first anchor", after: "after the last anchor", between: "between anchors" } as const;

export function NodeSpines({ rows, anchor, onToggle, onPick }: NodeSpinesView) {
  if (rows === "failed")
    return (
      <p className="text-xs text-destructive" data-testid="node-spines-failed">
        The graph’s spines could not be read.
      </p>
    );
  if (rows === null) return <p className="text-xs text-muted-foreground">Reading the graph’s spines…</p>;
  if (rows.length === 0) return <p className="text-xs text-muted-foreground" data-testid="node-spines-none">Its graph declares no spines.</p>;
  const nowhere = rows.every((r) => r.state === "ready" && r.place === null);
  return (
    <div className="space-y-2" data-testid="node-spines">
      {nowhere ? (
        <p className="text-xs text-muted-foreground" data-testid="node-spines-nowhere">
          On none of its graph’s spines.
        </p>
      ) : null}
      <ul className="space-y-1.5">
        {rows.map((r) => (
          <li key={r.name} className="flex items-start gap-2 text-sm" data-spine={r.name} data-testid="node-spine">
            <button
              type="button"
              aria-pressed={r.shown}
              onClick={() => onToggle(r.name)}
              title={`${r.title}: ${r.clock === "calendar" ? "time" : "order"} spine${r.main ? ", main" : ""}. ${r.shown ? "Click to hide it (force layout)" : "Click to lay the graph out along it"}`}
              className={cn(
                "flex h-7 shrink-0 items-center gap-1.5 rounded-full border px-2.5 text-xs transition-colors duration-150",
                r.shown ? "border-primary bg-primary text-primary-foreground" : "text-muted-foreground hover:bg-muted hover:text-foreground",
              )}
              data-testid="node-spine-toggle"
              data-on={r.shown || undefined}
            >
              <SpineIcon clock={r.clock} />
              {r.name}
            </button>
            {nowhere ? null : (
              <div className="min-w-0 flex-1 pt-1 text-xs" data-testid="node-spine-place">
                <Where r={r} anchor={anchor} onPick={onPick} />
              </div>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

function Where({ r, anchor, onPick }: { r: NodeOnSpine; anchor: NodeSpinesView["anchor"]; onPick: (key: string) => void }) {
  const quiet = (t: string) => <span className="text-muted-foreground">{t}</span>;
  if (r.state === "loading")
    return (
      <span className="flex items-center gap-1 text-muted-foreground">
        <Loader2 className="size-3 animate-spin" aria-hidden /> Placing…
      </span>
    );
  if (r.state === "failed") return <span className="text-destructive">Its placements could not be read.</span>;
  if (r.state === "unsupported") return quiet("This umx cannot place nodes on a spine yet.");
  if (r.state === "gone") return quiet("The graph no longer declares this spine.");
  if (!r.place) return quiet("Not on this spine.");
  const by = BY[r.place.by] ?? r.place.by;
  if (r.place.anchor) return <span>{quiet(`An anchor, ${by}`)}</span>;
  if (r.anchors.length === 0) return quiet(`${r.side ? SIDE[r.side] : "In a band with no anchor"}, ${by}`);
  return (
    <span className="break-words">
      {quiet("At ")}
      {r.anchors.map((id, i) => {
        const a = anchor(id);
        return (
          <span key={id}>
            {i > 0 ? quiet(", ") : null}
            <button type="button" className="font-medium hover:underline" onClick={() => onPick(a.key)} data-testid="node-spine-anchor">
              {a.title}
            </button>
          </span>
        );
      })}
      {quiet(`, ${by}`)}
    </span>
  );
}
