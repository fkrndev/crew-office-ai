/**
 * What a provider's agents can and cannot do inside this app, as DATA.
 *
 * The app grew up Claude-first: the cost HUD, the context gauge, token budgets,
 * auto-compaction, and the Skills tab all assume Claude's telemetry and file
 * layout. For every other engine some of those silently do nothing — which reads
 * as "broken" rather than "not available for this engine". This module is the one
 * place that says which is which, so the UI can show a short, honest note next to
 * the provider picker instead of scattering `provider === 'hermes'` checks.
 *
 * Everything is derived from the provider preset where the preset already knows
 * the answer (context commands, auto-mode flag); only facts the preset cannot
 * express (usage telemetry, where app-installed skills land, first-run setup)
 * live in the small tables below, each with the evidence for it.
 */
import { providerPreset, type AgentProvider } from './agentProvider';
import { contextCommandsForProvider } from './providerAutomation';

export interface ProviderCapabilities {
  /** The engine reports tokens / cost / context fill to the app. When false the
   *  cost HUD, the context gauge and token/USD budgets stay empty for its agents. */
  usageTelemetry: boolean;
  /** The app knows a typed command that compacts the context. */
  compactCommand: boolean;
  /** The app knows a typed command that clears the context. */
  clearCommand: boolean;
  /** Skills installed from the Skills tab (~/.claude/skills) are read by this
   *  engine. 'no' = verified it reads a different directory; 'unknown' = not
   *  verified either way, so the UI says nothing. */
  appSkills: 'yes' | 'no' | 'unknown';
  /** Auto mode maps to a flag (vs. nothing / config only). */
  autoModeFlag: string;
  /** i18n key of a one-line hint for the freeform model field, when the model id
   *  format is not obvious. */
  modelHintKey?: string;
  /** i18n key explaining what auto mode really does for this engine, when the
   *  generic "never pauses" wording would mislead. */
  autoModeNoteKey?: string;
  /** i18n key: what to do if the CLI immediately asks for provider setup. */
  firstRunHintKey?: string;
}

/** Engines known NOT to feed usage into the app: no OTel, no status-line shim,
 *  and the hook bridge payloads carry no token counts. Hermes: src-tauri
 *  bin/hive-hook.rs `--hermes` maps lifecycle events only. */
const NO_USAGE_TELEMETRY: ReadonlySet<AgentProvider> = new Set<AgentProvider>(['hermes']);

/** Skills land in ~/.claude/skills and <agent>/.claude/skills (src-tauri
 *  skills.rs). Hermes reads $HERMES_HOME/skills (+ `skills.external_dirs`). */
const APP_SKILLS: Partial<Record<AgentProvider, 'yes' | 'no'>> = {
  claude: 'yes',
  hermes: 'no'
};

const MODEL_HINT: Partial<Record<AgentProvider, string>> = {
  hermes: 'providerNotes.hermes.modelHint'
};
const AUTO_NOTE: Partial<Record<AgentProvider, string>> = {
  hermes: 'providerNotes.hermes.autoNote'
};
const FIRST_RUN_HINT: Partial<Record<AgentProvider, string>> = {
  hermes: 'providerNotes.hermes.firstRunHint'
};

export function capabilitiesForProvider(provider: AgentProvider): ProviderCapabilities {
  const ctx = contextCommandsForProvider(provider);
  return {
    usageTelemetry: !NO_USAGE_TELEMETRY.has(provider),
    compactCommand: ctx.compact !== null,
    clearCommand: ctx.clear !== null,
    appSkills: APP_SKILLS[provider] ?? 'unknown',
    autoModeFlag: providerPreset(provider).autoModeFlag ?? '',
    modelHintKey: MODEL_HINT[provider],
    autoModeNoteKey: AUTO_NOTE[provider],
    firstRunHintKey: FIRST_RUN_HINT[provider]
  };
}

/** i18n keys (all under `providerNotes.*`) of the notes worth showing for a
 *  provider, in display order. Empty for Claude — the app's native path has
 *  nothing to warn about. `autoMode` gates the auto-mode note. */
export function providerNoteKeys(
  provider: AgentProvider,
  opts: { autoMode: boolean }
): string[] {
  const c = capabilitiesForProvider(provider);
  const keys: string[] = [];
  if (c.firstRunHintKey) keys.push(c.firstRunHintKey);
  if (opts.autoMode && c.autoModeNoteKey && c.autoModeFlag) keys.push(c.autoModeNoteKey);
  if (!c.usageTelemetry) keys.push('providerNotes.noUsage');
  if (c.appSkills === 'no') keys.push('providerNotes.ownSkills');
  return keys;
}

/** Providers (preset order) that report no usage — for the budgets note. */
export function providersWithoutUsageTelemetry(): AgentProvider[] {
  return [...NO_USAGE_TELEMETRY];
}
