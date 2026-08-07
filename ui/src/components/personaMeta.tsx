// Single source of truth for how each persona is shown across the UI — its label,
// symbol, and color. Use this everywhere a persona appears (session header, session
// rows, composer, proposal cards) so persona identity reads instantly and
// consistently. Colors mirror the --persona-* tokens in styles.css.
import {
  Box,
  Blend,
  Clapperboard,
  Gamepad2,
  Lightbulb,
  LineChart,
  FlaskConical,
  ChevronDown,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";

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
  /** one-line description of what this agent does */
  role: string;
};

const META: Record<string, PersonaMeta> = {
  producer: {
    id: "producer",
    label: "Producer",
    Icon: Clapperboard,
    color: "var(--persona-producer)",
    cls: "persona-producer",
    role: "Orchestrates the funnel; suggests the next agent for you to approve.",
  },
  "idea-foundry": {
    id: "idea-foundry",
    label: "Idea foundry",
    Icon: Lightbulb,
    color: "var(--persona-idea-foundry)",
    cls: "persona-idea-foundry",
    role: "Interviews a vague seed into a thesis with a real hook.",
  },
  analyst: {
    id: "analyst",
    label: "Analyst",
    Icon: LineChart,
    color: "var(--persona-analyst)",
    cls: "persona-analyst",
    role: "Scores an idea against the 35-signal market-fit rubric.",
  },
  "game-designer": {
    id: "game-designer",
    label: "Game designer",
    Icon: Gamepad2,
    color: "var(--persona-game-designer)",
    cls: "persona-game-designer",
    role: "Builds the playable prototype — loop first, then polish.",
  },
  blender: {
    id: "blender",
    label: "Blender artist",
    Icon: Box,
    color: "var(--persona-blender)",
    cls: "persona-blender",
    role: "Models 3D assets in your open Blender and exports them for the build.",
  },
  comfyui: {
    id: "comfyui",
    label: "ComfyUI artist",
    Icon: Blend,
    color: "var(--persona-comfyui)",
    cls: "persona-comfyui",
    role: "Generates 2D art on your local ComfyUI and checks the result.",
  },
  research: {
    id: "research",
    label: "Research",
    Icon: FlaskConical,
    color: "var(--persona-research)",
    cls: "persona-research",
    role: "The general base agent. Fallback for non-game work.",
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
  "blender",
  "comfyui",
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

/** Clickable persona badge that opens a role-annotated dropdown. When `locked`
 *  (an already-started session, whose persona is fixed), it renders a plain
 *  badge instead. */
export function PersonaPicker({
  value,
  onChange,
  locked = false,
}: {
  value?: string | null;
  onChange: (id: string) => void;
  locked?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  if (locked) return <PersonaBadge persona={value} />;

  const m = personaMeta(value);
  const Icon = m.Icon;
  const selected = value ?? "research";
  return (
    <div className="persona-picker" ref={ref}>
      <button
        className={`persona-badge ${m.cls} persona-badge-btn`}
        onClick={() => setOpen((v) => !v)}
        title="Change persona"
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <Icon size={14} style={{ color: m.color }} />
        <span className="persona-badge-label">{m.label}</span>
        <ChevronDown size={13} className="persona-badge-chev" />
      </button>
      {open && (
        <div className="persona-menu" role="listbox">
          <div className="persona-menu-h">Start this session as</div>
          {PERSONA_ORDER.map((id) => {
            const pm = personaMeta(id);
            const OptIcon = pm.Icon;
            const isSel = id === selected;
            return (
              <button
                key={id}
                className={`persona-opt ${pm.cls} ${isSel ? "sel" : ""}`}
                role="option"
                aria-selected={isSel}
                onClick={() => {
                  onChange(id);
                  setOpen(false);
                }}
              >
                <span className="persona-opt-ic">
                  <OptIcon size={15} style={{ color: pm.color }} />
                </span>
                <span className="persona-opt-t">
                  <span className="persona-opt-n" style={{ color: pm.color }}>
                    {pm.label}
                    {id === DEFAULT_PERSONA && <span className="persona-opt-def">default</span>}
                  </span>
                  <span className="persona-opt-d">{pm.role}</span>
                </span>
                {isSel && <span className="persona-opt-check">✓</span>}
              </button>
            );
          })}
          <div className="persona-menu-foot">
            You rarely need this — the Producer routes you to the others as you go.
          </div>
        </div>
      )}
    </div>
  );
}
