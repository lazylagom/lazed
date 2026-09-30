import { describe, expect, it } from "vitest";
import type { FsEntry } from "../shared/lazed";
import { buildTree, filterEntries } from "./files-tree";

const e = (
  path: string,
  kind: "file" | "dir" = "file",
  git?: FsEntry["git"],
  collapsed?: boolean,
): FsEntry => ({
  path,
  kind,
  git: git ?? null,
  collapsed,
});

describe("buildTree", () => {
  it("nests files under dirs, dirs first", () => {
    const tree = buildTree([
      e("src/App.tsx"),
      e("src/lib/util.ts"),
      e("README.md"),
      e("src", "dir"),
      e("src/lib", "dir"),
    ]);
    expect(tree.map((n) => n.name)).toEqual(["src", "README.md"]);
    const src = tree[0];
    expect(src.kind).toBe("dir");
    expect(src.children.map((n) => n.name)).toEqual(["lib", "App.tsx"]);
    expect(src.children[0].children[0].path).toBe("src/lib/util.ts");
  });

  it("passes backend badges through untouched", () => {
    const tree = buildTree([
      e("a", "dir", "M"),
      e("a/dirty.ts", "file", "M"),
      e("ignored", "dir", "!", true),
    ]);
    expect(tree[0].git).toBe("M");
    expect(tree[1].git).toBe("!");
    expect(tree[1].collapsed).toBe(true);
  });

  it("handles a dir entry arriving after its children", () => {
    const tree = buildTree([e("a/b.ts"), e("a", "dir", "M")]);
    expect(tree[0].git).toBe("M");
    expect(tree[0].children[0].path).toBe("a/b.ts");
  });
});

describe("filterEntries", () => {
  const entries = [
    e("src/App.tsx"),
    e("src/styles.css"),
    e("README.md"),
    e("src", "dir"),
  ];
  it("matches case-insensitive substrings on the full path", () => {
    const out = filterEntries(entries, "app");
    expect(out.map((x) => x.path)).toEqual(["src/App.tsx"]);
    expect(filterEntries(entries, "SRC").map((x) => x.path)).toEqual([
      "src/App.tsx",
      "src/styles.css",
      "src",
    ]);
  });
  it("empty query matches nothing", () => {
    expect(filterEntries(entries, "  ")).toEqual([]);
  });
});
