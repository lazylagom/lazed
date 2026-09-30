import type { FsEntry, FsGit } from "../shared/lazed";

export interface TreeNode {
  name: string;
  path: string;
  kind: "file" | "dir";
  /** git badge — for dirs the backend already rolled up the worst
   * descendant state */
  git?: FsGit | null;
  /** backend-listed collapsed dir (ignored / embedded repo) — not
   * expandable, contents never arrive */
  collapsed?: boolean;
  children: TreeNode[];
}

const byDirThenName = (a: TreeNode, b: TreeNode) =>
  a.kind !== b.kind
    ? a.kind === "dir"
      ? -1
      : 1
    : a.name.localeCompare(b.name);

/** Flat repo-relative entries → nested tree, dirs first. `git` badges
 * pass through as the backend computed them (dirs carry the rollup). */
export function buildTree(entries: FsEntry[]): TreeNode[] {
  const nodes = new Map<string, TreeNode>();
  const roots: TreeNode[] = [];

  for (const e of entries) {
    const segs = e.path.split("/");
    let prefix = "";
    let parent: TreeNode[] = roots;
    for (let i = 0; i < segs.length; i++) {
      prefix = prefix ? `${prefix}/${segs[i]}` : segs[i];
      const last = i === segs.length - 1;
      let node = nodes.get(prefix);
      if (!node) {
        node = {
          name: segs[i],
          path: prefix,
          kind: last ? e.kind : "dir",
          git: last ? (e.git ?? undefined) : undefined,
          collapsed: last ? e.collapsed : undefined,
          children: [],
        };
        nodes.set(prefix, node);
        parent.push(node);
      } else if (last) {
        // a dir entry can arrive before/after its children's implicit nodes
        node.kind = e.kind;
        node.git = e.git ?? node.git;
        node.collapsed = e.collapsed;
      }
      parent = node.children;
    }
  }

  const sortAll = (list: TreeNode[]) => {
    list.sort(byDirThenName);
    for (const n of list) if (n.kind === "dir") sortAll(n.children);
  };
  sortAll(roots);
  return roots;
}

/** Names-mode filter — case-insensitive substring on the repo-relative path.
 * A dir match keeps the dir row itself; a file match is shown flat (no tree
 * context, like Orca's find list). */
export function filterEntries(entries: FsEntry[], q: string): FsEntry[] {
  const needle = q.trim().toLowerCase();
  if (!needle) return [];
  return entries.filter((e) => e.path.toLowerCase().includes(needle));
}
