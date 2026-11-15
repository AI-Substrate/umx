"use client";
// The FUI skins' readout card (K1, design note section 4, readability rule 3): every number is one the view already
// holds for the graph on screen (security K1-a). No fps meter and no clock that means nothing; the agent's age is
// the real time since its last event.
import { useEffect, useState } from "react";
import { cn, number } from "@/lib/utils";

export type ReadoutValues = {
  nodes: [shown: number, total: number];
  links: [shown: number, total: number];
  /** Groups of 4 or more and how they were found. */
  groups: { count: number; method: "louvain" | "parent" | "graph" } | null;
  /** Nodes the Show toggles hide (provenance and retired). */
  hidden: number;
  focus: { hops: number; nodes: number } | null;
  selected: number;
  agent: { seq: number; at_ms: number } | null;
  layout: { done: boolean; iterations?: number; ms?: number; capped?: boolean };
  /** Hold (plan 152): the open graphs' held changes (null: many); undefined while live. */
  pending?: number | null;
};

/** What the canvas adds: the camera's zoom, the glow and the callouts on screen. */
export type CanvasReadouts = { zoom: number; glow: [shown: number, wanted: number]; callouts: [shown: number, more: number] };

function Row({ label, children, testId }: { label: string; children: React.ReactNode; testId: string }) {
  return (
    <>
      <dt className="text-[var(--chrome-text)]">{label}</dt>
      <dd className="text-right text-[var(--readout)]" data-testid={`readout-${testId}`}>
        {children}
      </dd>
    </>
  );
}

const pair = ([a, b]: [number, number]) => `${number.format(a)}/${number.format(b)}`;

function useAge(at: number | null): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (at === null) return;
    const t = setInterval(() => setNow(Date.now()), 1_000);
    return () => clearInterval(t);
  }, [at]);
  return at === null ? null : Math.max(0, Math.floor((now - at) / 1_000));
}

export function Readouts({ values, canvas, className }: { values: ReadoutValues; canvas: CanvasReadouts; className?: string }) {
  const age = useAge(values.agent?.at_ms ?? null);
  const l = values.layout;
  return (
    <div className={cn("fui-frame fui-readout panel-in relative rounded-md border bg-card/90 px-3 py-2", className)} data-testid="readouts">
      <span className="fui-brackets" aria-hidden />
      <dl className="grid grid-cols-[auto_auto] gap-x-5 gap-y-0.5">
        <Row label="Nodes" testId="nodes">{pair(values.nodes)}</Row>
        <Row label="Links" testId="links">{pair(values.links)}</Row>
        <Row label="Groups" testId="groups">{values.groups ? `${number.format(values.groups.count)} ${values.groups.method}` : "-"}</Row>
        <Row label="Hidden" testId="hidden">{number.format(values.hidden)}</Row>
        <Row label="Focus" testId="focus">{values.focus ? `${values.focus.hops}h ${number.format(values.focus.nodes)}` : "-"}</Row>
        <Row label="Selected" testId="selected">{number.format(values.selected)}</Row>
        {values.pending !== undefined ? (
          <Row label="Pending" testId="pending">{values.pending === null ? "many" : number.format(values.pending)}</Row>
        ) : null}
        <Row label="Agent" testId="agent">{values.agent ? `#${values.agent.seq} ${age}s` : "-"}</Row>
        <Row label="Layout" testId="layout">{l.done ? `${number.format(l.iterations ?? 0)} it ${number.format(l.ms ?? 0)}ms` : "running"}</Row>
        <Row label="Capped" testId="capped">{l.done ? (l.capped ? "yes" : "no") : "-"}</Row>
        <Row label="Zoom" testId="zoom">{number.format(canvas.zoom)}%</Row>
        <Row label="Glow" testId="glow">{canvas.glow[1] > canvas.glow[0] ? `${canvas.glow[0]} of ${canvas.glow[1]}` : number.format(canvas.glow[0])}</Row>
        <Row label="Callouts" testId="callouts">{canvas.callouts[1] > 0 ? `${canvas.callouts[0]} +${canvas.callouts[1]}` : number.format(canvas.callouts[0])}</Row>
      </dl>
    </div>
  );
}
