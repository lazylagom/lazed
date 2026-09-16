import {
  Activity01Icon,
  FolderGitIcon,
  Settings02Icon,
  WorkflowIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";

export type RailView = "projects";

const ITEMS: { id: RailView; icon: typeof FolderGitIcon; label: string }[] = [
  { id: "projects", icon: FolderGitIcon, label: "projects (⇧⌘1)" },
];

/** Left edge icon rail — switches which panel occupies the sidebar space.
 * Automations, the session monitor, and settings are overlay actions
 * pinned to the bottom. */
export function Rail({
  active,
  onSelect,
  onAutomations,
  onSession,
  onSettings,
  automationAlert,
}: {
  active: RailView;
  onSelect: (v: RailView) => void;
  onAutomations: () => void;
  onSession: () => void;
  onSettings: () => void;
  automationAlert?: boolean;
}) {
  return (
    <div className="rail">
      {ITEMS.map((it) => (
        <button
          key={it.id}
          type="button"
          className={`rail-btn ${active === it.id ? "sel" : ""}`}
          title={it.label}
          onClick={() => onSelect(it.id)}
        >
          <HugeiconsIcon icon={it.icon} size={17} strokeWidth={1.5} />
        </button>
      ))}
      <div className="rail-bottom">
        <button
          type="button"
          className="rail-btn"
          title="automations (⇧⌘2)"
          onClick={onAutomations}
        >
          <HugeiconsIcon icon={WorkflowIcon} size={17} strokeWidth={1.5} />
          {automationAlert && <span className="rail-alert" />}
        </button>
        <button
          type="button"
          className="rail-btn"
          title="session monitor (⇧⌘3)"
          onClick={onSession}
        >
          <HugeiconsIcon icon={Activity01Icon} size={17} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className="rail-btn"
          title="settings (⌘,)"
          onClick={onSettings}
        >
          <HugeiconsIcon icon={Settings02Icon} size={17} strokeWidth={1.5} />
        </button>
      </div>
    </div>
  );
}
