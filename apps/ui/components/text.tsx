"use client";
import { Copy } from "lucide-react";
import { safeHttpUrl } from "@/lib/safe-url";

/** A umx id (26 Crockford characters) or address (`umx:<graph>/<id>`). */
const ID_LIKE = /^(umx:[a-z][a-z0-9_]{0,63}\/)?[0-9A-HJKMNP-TV-Z]{26}$/;
export const idLike = (value: string) => ID_LIKE.test(value);

/** Puts the whole value on the clipboard; a clipboard that is missing or refuses (no permission, not focused) is no
 * error: the value stays on screen to select by hand. */
export async function copyText(value: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(value);
    return true;
  } catch {
    return false;
  }
}

/** An id or address: mono, on one line, cut with an ellipsis when it does not fit, never broken mid-token, and a copy
 * button for the whole of it (ui-expert K1-N6). */
export function IdText({ value, className }: { value: string; className?: string }) {
  return (
    <span className={`flex min-w-0 items-center gap-1 ${className ?? ""}`} data-testid="id-text">
      <span className="min-w-0 truncate font-mono text-xs" title={value}>
        {value}
      </span>
      <button
        type="button"
        aria-label="Copy"
        className="shrink-0 text-muted-foreground hover:text-foreground"
        onClick={() => void copyText(value)}
        data-testid="copy-id"
      >
        <Copy className="size-3" />
      </button>
    </span>
  );
}

/** Graph content as plain text; a value that is wholly an http(s) URL becomes a link that opens a new tab. */
export function TextOrLink({ value, className }: { value: string; className?: string }) {
  const href = safeHttpUrl(value);
  if (!href) return <span className={className}>{value}</span>;
  return (
    <a href={href} target="_blank" rel="noopener noreferrer" className={`${className ?? ""} text-primary underline underline-offset-2 break-all`}>
      {value}
    </a>
  );
}

export function Value({ value }: { value: unknown }) {
  if (value === null || value === undefined) return <span className="text-muted-foreground">none</span>;
  if (typeof value === "string") return idLike(value) ? <IdText value={value} /> : <TextOrLink value={value} className="break-words" />;
  if (typeof value === "number" || typeof value === "boolean") return <span className="tabular-nums">{String(value)}</span>;
  if (Array.isArray(value) && value.every((v) => typeof v === "string" || typeof v === "number")) {
    return (
      <ul className="space-y-0.5">
        {value.map((v, i) => (
          <li key={i}>{typeof v === "string" && idLike(v) ? <IdText value={v} /> : <TextOrLink value={String(v)} className="break-words" />}</li>
        ))}
      </ul>
    );
  }
  return <pre className="overflow-x-auto rounded bg-muted px-2 py-1 font-mono text-[11px] whitespace-pre-wrap">{JSON.stringify(value, null, 2)}</pre>;
}
