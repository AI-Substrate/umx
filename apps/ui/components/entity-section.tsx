"use client";
// The node panel's Entity section (ws009 M2, plan 165, design point 5, security's ruling): the selected node's entity
// over the open graphs, members grouped by graph with how each joined, proposed partners, members in loaded graphs
// that are not open (never a secured one), denials that split it, and in merged mode the merged record (D69 (2)). Keys
// appear here only: a key is shown when umx's entity read gave it unflagged (F188, ws009 M2b), else as its plain scheme
// and "(hidden key)".
import { ChevronDown, ChevronRight, Loader2, Waypoints } from "lucide-react";
import { useState } from "react";
import { Value } from "@/components/text";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { EntityReply } from "@/lib/entity-read";
import { hiddenKeyText, type Match, type Merged, type Walked } from "@/lib/graph/entities";
import { unq } from "@/lib/graph/keys";
import type { ApiError } from "@/lib/types";

export type EntityView = {
  walked: Walked;
  /** Each member's title and updated time, by canvas key. */
  about: (key: string) => { title: string; updated: string | null };
  /** Graphs that hold two or more members (umx's entity_graph_twice). */
  twice: string[];
  reply: EntityReply | null;
  loading: boolean;
  error: ApiError | null;
  merged: Merged | null;
  onPick: (key: string) => void;
  onAcross: () => void;
  onOpen: (graph: string) => void;
};

/** "12 Nov 09:14", in the person's time zone; empty for none. */
export function when(iso: string | null | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" }).replace(",", "");
}

/** How a member joined, in words. A key is shown only when it is one of `shown`, the member's keys umx's entity read
 * gave unflagged (ws009 M2b); any other is its plain scheme and "(hidden key)". */
export function joinedText(matched: readonly Match[], shown: readonly string[] | null = null): string {
  const parts: string[] = [];
  if (matched.some((m) => m.by === "asked")) parts.push("this node");
  const keys = [...new Set(matched.flatMap((m) => (m.by === "key" ? [shown?.includes(m.key) ? m.key : hiddenKeyText(m.key)] : [])))];
  if (keys.length > 0) parts.push(`by key (${keys.join(", ")})`);
  for (const g of new Set(matched.flatMap((m) => (m.by === "link" ? [m.graph] : [])))) parts.push(`linked in ${g}`);
  return parts.join(" · ");
}

/** A member's keys as umx's entity read gave them, flagged ones already replaced on the server; null before it came. */
export function shownKeys(reply: EntityReply | null, graph: string, id: string): string[] | null {
  return reply?.members.find((m) => m.graph === graph && m.id === id)?.keys ?? null;
}

function MergedField({ name, value, graph, others }: Merged["fields"][number]) {
  const [open, setOpen] = useState(false);
  return (
    <div className="contents">
      <dt className="pt-0.5 font-mono text-xs text-muted-foreground">{name}</dt>
      <dd className="min-w-0 space-y-1">
        <div className="flex items-start gap-1.5">
          <div className="min-w-0 flex-1">
            <Value value={value} />
          </div>
          <Badge className="bg-card font-mono text-[10px]" data-testid="merged-chip">
            {graph}
          </Badge>
        </div>
        {others.length > 0 ? (
          <button type="button" className="flex items-center gap-0.5 text-xs text-muted-foreground hover:text-foreground" onClick={() => setOpen((o) => !o)} data-testid="merged-others">
            {open ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}+{others.length} other
          </button>
        ) : null}
        {open
          ? others.map((o, i) => (
              <div key={i} className="flex items-start gap-1.5 pl-3 text-muted-foreground">
                <div className="min-w-0 flex-1">
                  <Value value={o.value} />
                </div>
                <Badge className="bg-card font-mono text-[10px]">{o.graph}</Badge>
              </div>
            ))
          : null}
      </dd>
    </div>
  );
}

export function EntitySection({ walked, about, twice, reply, loading, error, merged, onPick, onAcross, onOpen }: EntityView) {
  const { members, proposed, denied } = walked;
  const graphs = [...new Set(members.map((m) => m.graph))];
  const kind = members[0]?.kind ?? "";
  return (
    <section
      className="space-y-3 border-t px-5 py-4"
      data-testid="entity-section"
      data-members={members.map((m) => m.key).join(",")}
      data-proposed={proposed.map((p) => p.key).join(",")}
      data-denied={denied.map((d) => d.key).join(",")}
    >
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="section-head" data-testid="entity-head">
          Entity · {kind} · {members.length} {members.length === 1 ? "member" : "members"} in {graphs.length} {graphs.length === 1 ? "graph" : "graphs"}
        </h3>
        {twice.map((g) => (
          <Badge
            key={g}
            className="border-amber-500/60 bg-card text-[10px]"
            title={`entity_graph_twice: ${g} holds two of its members; a key or a link probably joined two different things`}
            data-testid="graph-twice"
          >
            Twice in {g}
          </Badge>
        ))}
      </div>
      {members.length > 1 ? (
        <Button size="sm" variant="outline" onClick={onAcross} data-testid="show-across">
          <Waypoints /> Show across graphs
        </Button>
      ) : null}
      {merged ? (
        <div className="space-y-2 rounded-md border px-3 py-2" data-testid="merged-record">
          <p className="text-xs text-muted-foreground" data-testid="merged-from">
            title from {merged.from}
            {merged.updated ? ` · updated ${when(merged.updated)}` : ""}
          </p>
          {reply?.recordsLeftOut ? (
            <p className="text-xs text-muted-foreground" data-testid="merged-left-out">
              Merged from {reply.records?.length ?? 0} members; umx did not read {reply.recordsLeftOut} more.
            </p>
          ) : null}
          {merged.fields.length > 0 ? (
            <dl className="grid grid-cols-[minmax(80px,auto)_1fr] gap-x-3 gap-y-2 text-sm">
              {merged.fields.map((f) => (
                <MergedField key={f.name} {...f} />
              ))}
            </dl>
          ) : null}
        </div>
      ) : null}
      <div className="space-y-2">
        {graphs.map((g) => (
          <div key={g} className="space-y-1">
            <p className="font-mono text-[11px] text-muted-foreground">{g}</p>
            <ul className="space-y-1">
              {members
                .filter((m) => m.graph === g)
                .map((m) => {
                  const a = about(m.key);
                  return (
                    <li key={m.key} className="text-sm">
                      <button type="button" className="text-left hover:underline" onClick={() => onPick(m.key)} data-testid="entity-member">
                        {a.title}
                      </button>
                      <p className="text-xs text-muted-foreground" data-testid="entity-joined">
                        {joinedText(m.matched, shownKeys(reply, m.graph, unq(m.key).id))}
                        {a.updated ? ` · updated ${when(a.updated)}` : ""}
                      </p>
                    </li>
                  );
                })}
            </ul>
          </div>
        ))}
      </div>
      {proposed.length > 0 ? (
        <div className="space-y-1" data-testid="entity-proposed">
          <p className="text-xs font-medium">Proposed ({proposed.length})</p>
          <ul className="space-y-1">
            {proposed.map((p) => (
              <li key={p.key} className="text-sm">
                <button type="button" className="text-left hover:underline" onClick={() => onPick(p.key)}>
                  {about(p.key).title}
                </button>
                <p className="text-xs text-muted-foreground">
                  in {p.graph} · possibly_same_as in {p.graph_of_edge}
                </p>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {denied.length > 0 ? (
        <div className="space-y-1" data-testid="entity-denied">
          <p className="text-xs font-medium">Not the same as ({denied.length})</p>
          <ul className="space-y-1">
            {denied.map((d) => (
              <li key={d.key} className="text-sm">
                <button type="button" className="text-left hover:underline" onClick={() => onPick(d.key)}>
                  {about(d.key).title}
                </button>
                <p className="text-xs text-muted-foreground">
                  in {d.graph} · not_same_as in {d.graph_of_edge}
                </p>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {loading ? (
        <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Loader2 className="size-3 animate-spin" /> Asking umx about graphs not open…
        </p>
      ) : null}
      {error ? <p className="text-xs text-muted-foreground">{error.message}</p> : null}
      {(reply?.notOpen ?? []).map((n) => (
        <div key={n.graph} className="flex items-center gap-2 text-sm" data-testid="entity-not-open">
          <span>
            {n.count} more in {n.graph} (not open)
          </span>
          <Button size="sm" variant="outline" className="h-6 px-2" onClick={() => onOpen(n.graph)}>
            Open
          </Button>
        </div>
      ))}
    </section>
  );
}
