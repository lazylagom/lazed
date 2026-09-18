/** Browser-only preview harness for UI iteration without the Tauri runtime.
 * `bun run dev`, then open http://localhost:1420/preview.html */
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { AutomationEditor } from "./features/AutomationEditor";
import type { AutomationInput } from "./shared/automations";
import type { ProjectInfo } from "./shared/lazed";
import "./styles.css";

const PROJECTS: ProjectInfo[] = [
  {
    project_id: "p1",
    label: "lazed",
    repo_root: "/tmp/lazed",
    repo_key: "lazed",
    workspaces: [],
  },
  {
    project_id: "p2",
    label: "api-server",
    repo_root: "/tmp/api",
    repo_key: "api",
    workspaces: [],
  },
];

function Preview() {
  const [open, setOpen] = useState(true);
  const [last, setLast] = useState<AutomationInput | null>(null);
  return (
    <div
      style={{
        height: "100%",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        flexDirection: "column",
        gap: 12,
      }}
    >
      <button type="button" onClick={() => setOpen(true)}>
        open editor
      </button>
      {last && (
        <pre
          style={{
            fontSize: 11,
            color: "var(--text-3)",
            maxWidth: 560,
            whiteSpace: "pre-wrap",
          }}
        >
          {JSON.stringify(last, null, 2)}
        </pre>
      )}
      {open && (
        <AutomationEditor
          projects={PROJECTS}
          onSave={(i) => {
            setLast(i);
            setOpen(false);
          }}
          onDeleteSeen={() => {}}
          onClose={() => setOpen(false)}
        />
      )}
    </div>
  );
}

const rootEl = document.getElementById("root");
if (rootEl) createRoot(rootEl).render(<Preview />);
