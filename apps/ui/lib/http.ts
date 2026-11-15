import { NextResponse } from "next/server";
import { UmxError } from "@/lib/umx";

export function errorResponse(error: unknown): NextResponse {
  if (error instanceof UmxError) {
    return NextResponse.json({ error: error.toJSON() }, { status: error.httpStatus, headers: { "cache-control": "no-store" } });
  }
  return NextResponse.json(
    { error: { code: "ui_internal", message: "The view's server failed; see its terminal.", next: null } },
    { status: 500, headers: { "cache-control": "no-store" } },
  );
}

/** The largest request body an API route reads: 1000 node ids, quoted and separated, fit in well under this. */
export const BODY_MAX = 64 * 1024;

const tooBig = (max: number) => new UmxError("ui_body_too_big", `The request body is over ${max} bytes.`, null, 413);

/**
 * Reads a request body as UTF-8 text, refusing it (413) once more than `max` bytes have arrived: a declared
 * Content-Length over the cap is refused before reading, and a chunked body without one is read only up to the cap,
 * then cancelled. Request.text() would read any size.
 */
export async function readBody(req: Request, max: number = BODY_MAX): Promise<string> {
  const declared = req.headers.get("content-length");
  if (declared !== null && Number(declared) > max) throw tooBig(max);
  if (!req.body) return "";
  const reader = req.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > max) {
      await reader.cancel().catch(() => {});
      throw tooBig(max);
    }
    chunks.push(value);
  }
  const all = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.byteLength;
  }
  return new TextDecoder().decode(all);
}

export function ok(body: unknown): NextResponse {
  return NextResponse.json(body, { headers: { "cache-control": "no-store" } });
}
