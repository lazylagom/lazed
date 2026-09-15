import { describe, expect, it } from "vitest";
import { parseDiff } from "./DiffView";

const SAMPLE = `diff --git a/foo.ts b/foo.ts
index 123..456 100644
--- a/foo.ts
+++ b/foo.ts
@@ -1,3 +1,4 @@
 const a = 1;
+const b = 2;
 const c = 3;
diff --git a/bar.ts b/bar.ts
new file mode 100644
index 000..789
--- /dev/null
+++ b/bar.ts
@@ -0,0 +1,2 @@
+export const x = 1;
+export const y = 2;
`;

describe("parseDiff", () => {
  it("returns empty for empty diff", () => {
    expect(parseDiff("")).toEqual([]);
  });

  it("splits files on diff --git headers", () => {
    const files = parseDiff(SAMPLE);
    expect(files.map((f) => f.path)).toEqual(["foo.ts", "bar.ts"]);
  });

  it("classifies add/del/ctx/hunk lines and skips meta", () => {
    const foo = parseDiff(SAMPLE)[0];
    const kinds = foo.lines.map((l) => l.kind);
    expect(kinds).toEqual(["hunk", "ctx", "add", "ctx"]);
    expect(foo.lines.map((l) => l.text)).not.toContain("index 123..456 100644");
  });

  it("tracks new-side line numbers from hunk headers", () => {
    const foo = parseDiff(SAMPLE)[0];
    const addLine = foo.lines.find((l) => l.kind === "add");
    expect(addLine?.newNo).toBe(2); // hunk starts at +1, ctx consumed 1
    const lastCtx = foo.lines[foo.lines.length - 1];
    expect(lastCtx.newNo).toBe(3);
  });
});
