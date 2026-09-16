import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";
import notice from "../../NOTICE?raw";

export function About({ onClose }: { onClose: () => void }) {
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => {});
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="about-head">
          <span className="about-title">lazed</span>
          {version && <span className="about-ver">v{version}</span>}
        </div>
        <pre className="about-notice">{notice.trim()}</pre>
      </div>
    </div>
  );
}
