import { FolderImportIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";

function basename(p: string) {
  const parts = p.replace(/\/$/, "").split("/");
  return parts[parts.length - 1] || p;
}

export function ImportProject({
  onImport,
  onClose,
}: {
  onImport: (cwd?: string, label?: string) => void;
  onClose: () => void;
}) {
  const [path, setPath] = useState("");
  const [label, setLabel] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const browse = async () => {
    const dir = await open({
      directory: true,
      title: "import project",
    }).catch(() => null);
    if (typeof dir === "string") {
      setPath(dir);
      if (!label.trim()) setLabel(basename(dir));
    }
  };

  const submit = () => {
    const cwd = path.trim();
    onImport(cwd || undefined, label.trim() || basename(cwd) || undefined);
  };

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div
        className="modal"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Enter") submit();
        }}
      >
        <div className="import-head">
          <HugeiconsIcon icon={FolderImportIcon} size={14} strokeWidth={1.5} />
          <span>import project</span>
        </div>
        <div className="import-row">
          <input
            ref={inputRef}
            className="modal-input"
            placeholder="/path/to/project"
            value={path}
            onChange={(e) => setPath(e.target.value)}
          />
          <button type="button" onClick={browse}>
            browse…
          </button>
        </div>
        <input
          className="modal-input"
          placeholder="label (defaults to folder name)"
          value={label}
          onChange={(e) => setLabel(e.target.value)}
        />
        <button type="button" className="fanout-go" onClick={submit}>
          {path.trim() ? "import" : "new empty workspace"}
        </button>
      </div>
    </div>
  );
}
