import {
  Folder01Icon,
  InboxIcon,
  Pulse01Icon,
  Settings01Icon,
  ZapIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";

export type RailView = "projects" | "inbox";

type RailItem = {
  icon: typeof Folder01Icon;
  title: string;
  desc: string;
  kbd: string;
};

const ITEMS: (RailItem & { id: RailView })[] = [
  {
    id: "projects",
    icon: Folder01Icon,
    title: "Projects",
    desc: "Workspaces, worktrees & agents",
    kbd: "⇧⌘1",
  },
  {
    id: "inbox",
    icon: InboxIcon,
    title: "Inbox",
    desc: "Captured items awaiting triage",
    kbd: "⇧⌘4",
  },
];

function RailButton({
  item,
  sel,
  alert,
  count,
  onClick,
}: {
  item: RailItem;
  sel?: boolean;
  alert?: boolean;
  count?: number;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`rail-btn ${sel ? "sel" : ""}`}
      aria-label={item.title}
      onClick={onClick}
    >
      <HugeiconsIcon icon={item.icon} size={17} strokeWidth={1.5} />
      {alert && <span className="rail-alert" />}
      {count !== undefined && count > 0 && (
        <span className="rail-count">{count > 99 ? "99+" : count}</span>
      )}
      <span className="rail-tip">
        <span className="rail-tip-row">
          <span className="rail-tip-title">{item.title}</span>
          <kbd className="rail-tip-kbd">{item.kbd}</kbd>
        </span>
        <span className="rail-tip-desc">{item.desc}</span>
      </span>
    </button>
  );
}

/** Left edge icon rail — switches which panel occupies the sidebar space.
 * Automations, the session monitor, and settings are screens pinned to
 * the bottom; the rail stays exposed while one is open. */
export function Rail({
  active,
  onSelect,
  onAutomations,
  onSession,
  onSettings,
  automationsOpen,
  sessionOpen,
  settingsOpen,
  automationAlert,
  inboxCount,
}: {
  active: RailView;
  onSelect: (v: RailView) => void;
  onAutomations: () => void;
  onSession: () => void;
  onSettings: () => void;
  automationsOpen?: boolean;
  sessionOpen?: boolean;
  settingsOpen?: boolean;
  automationAlert?: boolean;
  /** open inbox items — shown as a badge on the inbox button */
  inboxCount?: number;
}) {
  return (
    <div className="rail">
      {ITEMS.map((it) => (
        <RailButton
          key={it.id}
          item={it}
          sel={active === it.id}
          count={it.id === "inbox" ? inboxCount : undefined}
          onClick={() => onSelect(it.id)}
        />
      ))}
      <div className="rail-bottom">
        <RailButton
          item={{
            icon: ZapIcon,
            title: "Automations",
            desc: "Scheduled & recurring agent runs",
            kbd: "⇧⌘2",
          }}
          sel={automationsOpen}
          alert={automationAlert}
          onClick={onAutomations}
        />
        <RailButton
          item={{
            icon: Pulse01Icon,
            title: "Session Monitor",
            desc: "Live agent status across workspaces",
            kbd: "⇧⌘3",
          }}
          sel={sessionOpen}
          onClick={onSession}
        />
        <RailButton
          item={{
            icon: Settings01Icon,
            title: "Settings",
            desc: "Shortcuts & preferences",
            kbd: "⌘,",
          }}
          sel={settingsOpen}
          onClick={onSettings}
        />
      </div>
    </div>
  );
}
