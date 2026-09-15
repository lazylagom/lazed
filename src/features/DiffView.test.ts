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

  it("keeps +++/--- content lines inside hunks", () => {
    // meta prefixes only apply before the first @@ — inside a hunk,
    // `+++i;` is an added `++i;` line, `---x;` a removed `--x;` line.
    const d = `diff --git a/f.c b/f.c
index 1..2 100644
--- a/f.c
+++ b/f.c
@@ -1,3 +1,4 @@
 int main() {
+++i;
---x;
 }
`;
    const f = parseDiff(d)[0];
    expect(f.lines.map((l) => l.kind)).toEqual([
      "hunk",
      "ctx",
      "add",
      "del",
      "ctx",
    ]);
    expect(f.lines[2]).toMatchObject({ kind: "add", text: "+++i;", newNo: 2 });
    expect(f.lines[4].newNo).toBe(3); // } — numbering unaffected
  });

  it("does not charge a line number to \\-annotations", () => {
    const d = `diff --git a/f b/f
index 1..2 100644
--- a/f
+++ b/f
@@ -1,2 +1,2 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file
`;
    const f = parseDiff(d)[0];
    expect(f.lines.map((l) => l.kind)).toEqual([
      "hunk",
      "del",
      "meta",
      "add",
      "meta",
    ]);
    expect(f.lines[3].newNo).toBe(1); // +new is still line 1
  });
});
