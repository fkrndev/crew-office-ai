import { isAgentProvider, type AgentProvider } from './agentProvider';
import { type AccentColorName } from '@/design/tokens';

/**
 * A saved spawn preset — one-click hire. Captures everything the Add Agent
 * modal would set (provider, model, assembled command, identity, briefing,
 * isolation) so a repeat hire is a single click instead of re-filling 10
 * fields. Stored on `config.agentTemplates` — a raw (untyped) config key, so
 * it survives the typed-Config save round-trip the same way officeTheme and
 * terminalTheme do.
 *
 * `command` is stored verbatim (the assembled spawn line the user had in the
 * field), NOT rebuilt on apply — a template preserves deliberate power-user
 * edits. `model` is kept separately so the picker still shows the chip.
 */
export interface AgentTemplate {
  /** Stable id — crypto.randomUUID() at save time. */
  id: string;
  /** Display name on the picker chip ("Backend reviewer"). */
  label: string;
  provider: AgentProvider;
  /** Model slug for the picker chip; undefined = provider default. */
  model?: string;
  /** The assembled spawn command (verbatim from the command field). */
  command: string;
  /** Prefills the agent's display name (user can still rename). */
  name?: string;
  /** Role line → hive registry + prompt briefing. */
  description?: string;
  /** Standing goal → hive briefing. */
  goal?: string;
  /** Git worktree isolation on spawn. */
  isolate?: boolean;
  character?: string;
  accent?: AccentColorName;
}

const MAX_LABEL = 64;
const MAX_FIELD = 4000;

function cap(s: string, max: number): string {
  return s.length > max ? s.slice(0, max) : s;
}

/**
 * Shape-check an untrusted config value into a template list. Bad entries are
 * dropped silently (config.json is user-editable). The command must be a
 * non-empty string and the provider must be a known id — a template that
 * can't spawn is worse than none.
 */
export function sanitizeTemplates(raw: unknown): AgentTemplate[] {
  if (!Array.isArray(raw)) return [];
  const out: AgentTemplate[] = [];
  for (const t of raw) {
    if (typeof t !== 'object' || t === null) continue;
    const o = t as Record<string, unknown>;
    const label = typeof o.label === 'string' ? cap(o.label.trim(), MAX_LABEL) : '';
    const command = typeof o.command === 'string' ? cap(o.command.trim(), MAX_FIELD) : '';
    const provider = isAgentProvider(o.provider) ? o.provider : undefined;
    if (!label || !command || !provider) continue;
    out.push({
      id: typeof o.id === 'string' && o.id ? o.id : crypto.randomUUID(),
      label,
      provider,
      model: typeof o.model === 'string' ? o.model : undefined,
      command,
      name: typeof o.name === 'string' ? cap(o.name, MAX_LABEL) : undefined,
      description:
        typeof o.description === 'string' ? cap(o.description, MAX_FIELD) : undefined,
      goal: typeof o.goal === 'string' ? cap(o.goal, MAX_FIELD) : undefined,
      isolate: o.isolate === true,
      character: typeof o.character === 'string' ? o.character : undefined,
      accent: typeof o.accent === 'string' ? (o.accent as AccentColorName) : undefined
    });
  }
  return out;
}
