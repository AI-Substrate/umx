"use client";
// The agent's activity strip, its history drawer and the "Agent wants to show X" prompt (plan 114, design note
// section 5). The label ("<who> focused") is fixed chrome; the agent's note and any title are separate text elements,
// isolated with <bdi>, so the agent's words can neither spoof the label nor reorder it (security T2).
import { Bot, History, Undo2, X } from "lucide-react";
import { useState } from "react";
import type { Described } from "@/lib/agent";
import type { ApiError } from "@/lib/types";
import { cn } from "@/lib/utils";

export type StripEntry = { seq: number; at_ms: number; d: Described; replay: boolean };

/** An agent's open (ws009 M1b1): fixed words naming graphs by alias, and the person's choices. */
export type OpenBox = { lead: string; graphs: string; tail: string; actions: { label: string; run: () => void; id: string }[] };

const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });

function Line({ d }: { d: Described }) {
  return (
    <span className="min-w-0">
      <span className="font-medium" data-testid="agent-label">
        {d.who} {d.action}
      </span>{" "}
      {d.target ? (
        <bdi className="text-foreground" data-testid="agent-target">
          {d.target}
        </bdi>
      ) : null}
      {d.note !== null ? (
        <>
          <span className="text-muted-foreground">: </span>
          <bdi className="text-muted-foreground italic" data-testid="agent-note">
            {d.note}
          </bdi>
        </>
      ) : null}
    </span>
  );
}

export function AgentStrip({
  history,
  onUndo,
  prompt,
  onGo,
  onDismiss,
  onReplay,
  error,
  notice = null,
  onNoticeDismiss,
  openBox = null,
  onOpenDismiss,
}: {
  /** Newest first. */
  history: StripEntry[];
  /** Undo the agent's latest filter, when it is still on the chips. */
  onUndo: (() => void) | null;
  /** A camera move the agent asked for that waits for the person. */
  prompt: { target: string } | null;
  onGo: () => void;
  onDismiss: () => void;
  onReplay: (seq: number) => void;
  error: ApiError | null;
  /** Hold (plan 152): fixed words about held changes, with an action (a graph's Apply) when there is one. */
  notice?: { text: string; action?: { label: string; run: () => void } } | null;
  onNoticeDismiss?: () => void;
  openBox?: OpenBox | null;
  onOpenDismiss?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const latest = history[0];
  if (!latest && !prompt && !error && !notice && !openBox) return null;
  return (
    <div className="pointer-events-none absolute bottom-4 left-4 z-10 flex max-w-[min(46rem,calc(100%-6rem))] flex-col gap-2" data-testid="agent-strip">
      {open && history.length > 0 ? (
        <ol className="fui-frame pointer-events-auto max-h-64 space-y-1 overflow-y-auto rounded-lg border bg-card/95 p-2 text-xs shadow-sm backdrop-blur" data-testid="agent-history">
          {history.map((h) => (
            <li key={h.seq}>
              <button
                type="button"
                disabled={!h.replay}
                onClick={() => onReplay(h.seq)}
                className={cn("flex w-full items-baseline gap-2 rounded px-1.5 py-1 text-left", h.replay && "hover:bg-muted")}
                title={h.replay ? "Show it again" : undefined}
              >
                <span className="shrink-0 text-muted-foreground tabular-nums">{time(h.at_ms)}</span>
                <Line d={h.d} />
              </button>
            </li>
          ))}
        </ol>
      ) : null}
      {prompt ? (
        <div className="pointer-events-auto flex items-center gap-2 rounded-lg border border-[var(--canvas-agent)] bg-card px-3 py-1.5 text-sm shadow-sm" data-testid="agent-prompt">
          <Bot className="size-4 shrink-0 text-[var(--canvas-agent)]" aria-hidden />
          <span className="min-w-0">
            Agent wants to show <bdi className="font-medium">{prompt.target}</bdi>
          </span>
          <button type="button" className="rounded-md bg-[var(--canvas-agent)] px-2 py-0.5 text-xs font-medium text-white" onClick={onGo} data-testid="agent-go">
            Go
          </button>
          <button type="button" className="text-muted-foreground hover:text-foreground" aria-label="Dismiss" onClick={onDismiss}>
            <X className="size-4" />
          </button>
        </div>
      ) : null}
      {openBox ? (
        <div className="pointer-events-auto flex items-center gap-2 self-start rounded-lg border border-[var(--canvas-agent)] bg-card px-3 py-1.5 text-sm shadow-sm" data-testid="agent-open">
          <Bot className="size-4 shrink-0 text-[var(--canvas-agent)]" aria-hidden />
          <span className="min-w-0" data-testid="agent-open-text">
            {openBox.lead} <bdi className="font-medium">{openBox.graphs}</bdi>
            {openBox.tail}
          </span>
          {openBox.actions.map((a) => (
            <button key={a.id} type="button" className="shrink-0 rounded-md bg-[var(--canvas-agent)] px-2 py-0.5 text-xs font-medium text-white" onClick={a.run} data-testid={`agent-open-${a.id}`}>
              {a.label}
            </button>
          ))}
          {onOpenDismiss ? (
            <button type="button" className="text-muted-foreground hover:text-foreground" aria-label="Dismiss" onClick={onOpenDismiss} data-testid="agent-open-dismiss">
              <X className="size-4" />
            </button>
          ) : null}
        </div>
      ) : null}
      {notice ? (
        <div className="pointer-events-auto flex items-center gap-2 self-start rounded-lg border border-amber-500/60 bg-card px-3 py-1.5 text-sm shadow-sm fui:font-mono fui:text-xs" data-testid="hold-notice">
          <span className="size-2 shrink-0 rounded-full bg-amber-500" aria-hidden />
          <span className="min-w-0">{notice.text}</span>
          {notice.action ? (
            <button type="button" className="shrink-0 rounded-md border px-1.5 py-0.5 text-xs hover:bg-muted" onClick={notice.action.run} data-testid="hold-notice-action">
              {notice.action.label}
            </button>
          ) : null}
          {onNoticeDismiss ? (
            <button type="button" className="text-muted-foreground hover:text-foreground" aria-label="Dismiss" onClick={onNoticeDismiss}>
              <X className="size-4" />
            </button>
          ) : null}
        </div>
      ) : null}
      {latest ? (
        <div className="fui-frame pointer-events-auto relative flex items-center gap-2 rounded-lg border bg-card/95 px-3 py-1.5 text-sm shadow-sm backdrop-blur" data-testid="agent-latest" data-seq={latest.seq}>
          <span className="fui-brackets" aria-hidden />
          <span className="flex shrink-0 items-center gap-1 rounded-full bg-[var(--canvas-agent)] px-1.5 py-0.5 text-[10px] font-semibold text-white uppercase">
            <Bot className="size-3" aria-hidden /> Agent
          </span>
          <Line d={latest.d} />
          {/* A node only in held changes is said once, by the hold notice (ui-expert N1). */}
          {latest.d.notShown > 0 && !notice?.action ? (
            <span className="shrink-0 text-xs text-amber-700 dark:text-amber-300" data-testid="agent-not-shown">
              agent named a node that is not shown
            </span>
          ) : null}
          {onUndo ? (
            <button type="button" className="flex shrink-0 items-center gap-1 rounded-md border px-1.5 py-0.5 text-xs hover:bg-muted" onClick={onUndo} data-testid="agent-undo">
              <Undo2 className="size-3" /> Undo
            </button>
          ) : null}
          <button
            type="button"
            className={cn("shrink-0 rounded-md p-1 text-muted-foreground hover:bg-muted", open && "bg-muted text-foreground")}
            aria-label="Agent history"
            aria-pressed={open}
            onClick={() => setOpen((o) => !o)}
            data-testid="agent-history-toggle"
          >
            <History className="size-4" />
          </button>
        </div>
      ) : null}
      {error ? (
        <p className="pointer-events-auto rounded-md bg-card/90 px-2 py-1 text-[11px] text-muted-foreground" data-testid="agent-error">
          Agent channel: {error.message}
        </p>
      ) : null}
    </div>
  );
}
