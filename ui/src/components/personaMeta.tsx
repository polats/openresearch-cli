// Single source of truth for how each persona is shown across the UI — its label,
// symbol, and color. Use this everywhere a persona appears (session header, session
// rows, composer, proposal cards) so persona identity reads instantly and
// consistently. Colors mirror the --persona-* tokens in styles.css.
import {
  Clapperboard,
  Gamepad2,
  Lightbulb,
  LineChart,
  FlaskConical,
  type LucideIcon,
} from "lucide-react";

export type PersonaMeta = {
  /** wire id */
  id: string;
  /** full label, e.g. "Game designer" */
  label: string;
  /** the persona's symbol */
  Icon: LucideIcon;
  /** css var for the persona's accent color */
  color: string;
  /** persona wire id used as a class suffix, e.g. persona-game-designer */
  cls: string;
};

const META: Record<string, PersonaMeta> = {
  producer: {
    id: "producer",
    label: "Producer",
    Icon: Clapperboard,
    color: "var(--persona-producer)",
    cls: "persona-producer",
  },
  "idea-foundry": {
    id: "idea-foundry",
    label: "Idea foundry",
    Icon: Lightbulb,
    color: "var(--persona-idea-foundry)",
    cls: "persona-idea-foundry",
  },
  analyst: {
    id: "analyst",
    label: "Analyst",
    Icon: LineChart,
    color: "var(--persona-analyst)",
    cls: "persona-analyst",
  },
  "game-designer": {
    id: "game-designer",
    label: "Game designer",
    Icon: Gamepad2,
    color: "var(--persona-game-designer)",
    cls: "persona-game-designer",
  },
  research: {
    id: "research",
    label: "Research",
    Icon: FlaskConical,
    color: "var(--persona-research)",
    cls: "persona-research",
  },
};

/** Resolve persona meta; null/unknown falls back to research (the base agent). */
export function personaMeta(persona?: string | null): PersonaMeta {
  return META[persona ?? "research"] ?? META.research;
}

/** The personas a human picks from, in pipeline order. Producer leads; research
 *  stays as the fallback base agent at the end. */
export const PERSONA_ORDER = [
  "producer",
  "idea-foundry",
  "analyst",
  "game-designer",
  "research",
] as const;

/** Default persona for a new game session — the orchestrator that leads the
 *  pipeline; other personas are reached via its handoffs/subagents. */
export const DEFAULT_PERSONA = "producer";

/** A small persona badge: colored symbol + optional label. */
export function PersonaBadge({
  persona,
  showLabel = true,
  compact = false,
  size = 14,
}: {
  persona?: string | null;
  showLabel?: boolean;
  compact?: boolean;
  size?: number;
}) {
  const m = personaMeta(persona);
  const Icon = m.Icon;
  return (
    <span
      className={`persona-badge ${m.cls}${compact ? " compact" : ""}`}
      title={`${m.label} persona`}
    >
      <Icon size={size} style={{ color: m.color }} />
      {showLabel && !compact && <span className="persona-badge-label">{m.label}</span>}
    </span>
  );
}
