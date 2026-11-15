// An agent's `umx ui open --raise` in a tab that is already connected (ws009 M1b2, ui-expert 2026-11-12): no new tab;
// this one marks its title ("● umx: agent wants to show you <graphs>") and asks for focus, which browsers often
// refuse. The title comes back once the person looks: on focus or when the tab turns visible, or after RAISE_MS when
// it already had focus.
export const RAISE_MS = 8_000;

type Listen = { addEventListener(type: string, f: () => void): void; removeEventListener(type: string, f: () => void): void };
export type RaiseDoc = Listen & { title: string; visibilityState: string; hasFocus(): boolean };
export type RaiseWin = Listen & { focus(): void };

export class Raiser {
  private before: string | null = null;
  private undo: (() => void) | null = null;

  constructor(
    private readonly doc: RaiseDoc,
    private readonly win: RaiseWin,
    private readonly later: (f: () => void, ms: number) => unknown = (f, ms) => setTimeout(f, ms),
  ) {}

  raise(title: string): void {
    this.undo?.();
    this.before ??= this.doc.title;
    this.doc.title = title;
    try {
      this.win.focus();
    } catch {
      // Refused: the title still shows.
    }
    const back = () => {
      this.undo?.();
      if (this.before !== null) this.doc.title = this.before;
      this.before = null;
    };
    if (this.doc.visibilityState === "visible" && this.doc.hasFocus()) {
      let live = true;
      this.undo = () => {
        live = false;
        this.undo = null;
      };
      this.later(() => {
        if (live) back();
      }, RAISE_MS);
      return;
    }
    const seen = () => {
      if (this.doc.visibilityState === "visible") back();
    };
    this.win.addEventListener("focus", back);
    this.doc.addEventListener("visibilitychange", seen);
    this.undo = () => {
      this.win.removeEventListener("focus", back);
      this.doc.removeEventListener("visibilitychange", seen);
      this.undo = null;
    };
  }
}
