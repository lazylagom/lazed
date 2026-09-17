import { FolderIcon, MultiplicationSignIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect } from "react";
import { Modal } from "../components/Modal";

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
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const browse = async () => {
    const dir = await open({
      directory: true,
      title: "add a project",
    }).catch(() => null);
    if (typeof dir === "string") onImport(dir, basename(dir));
  };

  return (
    <Modal onClose={onClose} className="addproj">
      <div className="addproj-head">
        <span className="addproj-title">Add a project</span>
        <button
          type="button"
          className="addproj-x"
          onClick={onClose}
          aria-label="close"
        >
          <HugeiconsIcon
            icon={MultiplicationSignIcon}
            size={18}
            strokeWidth={1.5}
          />
        </button>
      </div>
      <button type="button" className="addproj-card" onClick={browse}>
        <span className="addproj-card-ico">
          <HugeiconsIcon icon={FolderIcon} size={22} strokeWidth={1.4} />
        </span>
        <span className="addproj-card-text">
          <span className="addproj-card-title">Browse folder</span>
          <span className="addproj-card-sub">
            Local project, Git repo, or folder with many repos
          </span>
        </span>
      </button>
    </Modal>
  );
}
