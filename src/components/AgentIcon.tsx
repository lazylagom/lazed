import { BotIcon, TerminalIcon } from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import amp from "../assets/agents/amp.svg";
import claude from "../assets/agents/claude.svg";
import codex from "../assets/agents/codex.svg";
import cursor from "../assets/agents/cursor.svg";
import devin from "../assets/agents/devin.svg";
import gemini from "../assets/agents/gemini.svg";
import copilot from "../assets/agents/githubcopilot.svg";
import kimi from "../assets/agents/kimi.svg";
import opencode from "../assets/agents/opencode.svg";
import pi from "../assets/agents/pi.svg";
import qwen from "../assets/agents/qwen.svg";

const logos = new Map<string, string>([
  ["amp", amp],
  ["claude", claude],
  ["codex", codex],
  ["copilot", copilot],
  ["cursor", cursor],
  ["devin", devin],
  ["gemini", gemini],
  ["kimi", kimi],
  ["opencode", opencode],
  ["pi", pi],
  ["qwen", qwen],
]);

export function AgentIcon({ agent }: { agent?: string | null }) {
  const kind = agent?.trim().toLowerCase();
  const logo = kind ? logos.get(kind) : undefined;

  if (logo) {
    return (
      <span
        className="side-ico agent-icon"
        style={{ maskImage: `url("${logo}")` }}
        title={kind}
        aria-hidden="true"
      />
    );
  }

  return (
    <HugeiconsIcon
      icon={kind ? BotIcon : TerminalIcon}
      size={12}
      strokeWidth={1.5}
      className="side-ico"
      aria-hidden="true"
    />
  );
}
