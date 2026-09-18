interface DiffLine {
  kind: "ctx" | "add" | "del" | "hunk" | "meta";
  text: string;
  newNo?: number;
}

interface DiffFile {
  path: string;
  lines: DiffLine[];
}

export function parseDiff(diff: string): DiffFile[] {
  const files: DiffFile[] = [];
  let cur: DiffFile | null = null;
  let newNo = 0;
  // File meta (index/---/+++/mode/rename/binary lines) only exists between
  // the `diff --git` header and the first `@@` hunk — inside a hunk the
  // same prefixes are real content lines and must be kept.
  let inHunk = false;
  for (const raw of diff.split("\n")) {
    if (raw.startsWith("diff --git")) {
      const m = / b\/(.+)$/.exec(raw);
      cur = { path: m?.[1] ?? "?", lines: [] };
      files.push(cur);
      inHunk = false;
    } else if (cur) {
      if (raw.startsWith("@@")) {
        const m = /\+(\d+)/.exec(raw);
        newNo = m ? Number(m[1]) : 0;
        cur.lines.push({ kind: "hunk", text: raw });
        inHunk = true;
      } else if (!inHunk) {
        // file meta — not rendered
      } else if (raw.startsWith("\\")) {
        // "\ No newline at end of file" annotates the previous line; it
        // owns no line number, so don't let it consume newNo.
        cur.lines.push({ kind: "meta", text: raw });
      } else if (raw.startsWith("+")) {
        cur.lines.push({ kind: "add", text: raw, newNo: newNo++ });
      } else if (raw.startsWith("-")) {
        cur.lines.push({ kind: "del", text: raw });
      } else if (raw) {
        cur.lines.push({ kind: "ctx", text: raw, newNo: newNo++ });
      }
    }
  }
  return files;
}
