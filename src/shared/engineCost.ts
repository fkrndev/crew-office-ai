/**
 * Qualitative cost hints for the engine pickers.
 *
 * Deliberately words, never prices: what an engine costs depends on the account
 * and model behind it, and a number printed here would be wrong the day a plan
 * changes. Hermes is the recommended default because it can run on a low-cost
 * key (OpenRouter / DeepSeek); Claude bills against the user's Claude plan or API
 * credits, which is the expensive way to run a whole team.
 *
 * Returns i18n keys (under `engineCost.*`) so the copy stays translated; engines
 * with nothing honest to say return null and show no hint.
 */
import type { AgentProvider } from './agentProvider';

export interface EngineCostHint {
  /** Short tag shown on the picker chip. */
  tagKey: string;
  /** One sentence shown for the selected engine. */
  descKey: string;
  /** True for the engine we steer new users to. */
  recommended: boolean;
}

const HINTS: Partial<Record<AgentProvider, EngineCostHint>> = {
  hermes: {
    tagKey: 'engineCost.hermes.tag',
    descKey: 'engineCost.hermes.desc',
    recommended: true
  },
  claude: {
    tagKey: 'engineCost.claude.tag',
    descKey: 'engineCost.claude.desc',
    recommended: false
  },
  codex: {
    tagKey: 'engineCost.codex.tag',
    descKey: 'engineCost.codex.desc',
    recommended: false
  }
};

export function engineCostHint(provider: AgentProvider): EngineCostHint | null {
  return HINTS[provider] ?? null;
}
