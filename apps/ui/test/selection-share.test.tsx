// The top bar's selection sharing (components/selection-share.tsx, plan 117): "Agent can see your selection (n)"
// whenever a selection is shared, never while paused or empty, and the pause toggle always beside it.
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { SelectionShare } from "@/components/selection-share";
import type { ShareFailure } from "@/lib/selection";

const none = () => {};
const down = { graph: "demo", error: { code: "daemon_not_running", message: "The umx daemon is not running." } };
const html = (count: number, sharing: boolean, failures: ShareFailure[] = []) =>
  renderToStaticMarkup(<SelectionShare count={count} sharing={sharing} failures={failures} onSharing={none} />);

describe("the selection indicator", () => {
  it("shows the count whenever the selection is shared", () => {
    expect(html(3, true)).toMatch(/Agent can see your selection \(<span data-testid="selection-count">3<\/span>\)/);
    expect(html(1000, true)).toContain('data-testid="selection-count">1,000<');
  });

  it("says nothing about agents when the selection is empty or paused, and keeps the toggle", () => {
    for (const [count, sharing] of [[0, true], [3, false], [0, false]] as const) {
      const h = html(count, sharing);
      expect(h).not.toContain("Agent can see");
      expect(h).not.toContain("selection-error");
      expect(h).toContain('data-testid="share-selection"');
      expect(h).toContain(`data-sharing="${sharing ? "on" : "paused"}"`);
    }
    expect(html(3, false)).toMatch(/aria-checked="false"/);
    expect(html(3, true)).toMatch(/aria-checked="true"/);
  });

  it("shows a failed publish or clear whatever the count or the toggle, saying agents may still see it (R235-2)", () => {
    for (const [count, sharing] of [[3, true], [3, false], [0, false], [0, true]] as const) {
      const h = html(count, sharing, [down]);
      expect(h).not.toContain("Agent can see");
      expect(h).toContain("they may still see your previous selection");
      expect(h).toContain('data-code="daemon_not_running"');
      expect(h).toContain('data-graphs="demo"');
      expect(h).toContain('data-testid="share-selection"');
    }
    expect(html(0, false, [down, { ...down, graph: "alpha" }])).toContain("<bdi>demo, alpha</bdi>");
  });
});
