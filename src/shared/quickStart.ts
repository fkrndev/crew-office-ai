/**
 * First-run "quick start" — the one-step "Connect your AI" path of onboarding
 * (see components/QuickStart.tsx). A non-technical person pastes ONE API key and
 * the orchestrator runs on Hermes against that provider; no engine/model picking.
 *
 * Everything here is pure (the only bridge touch is the injected
 * `providerKeySet`/`providerKeyValidate`) so what the step validates and what it
 * WRITES is unit-testable with a fake bridge — never the real credential store.
 *
 * The key is stored through the SAME path Settings → AI Engines uses
 * (`providerKeySet({ backend, key })` → provider_key_set → `PROVIDER_KEY_<ID>` in
 * the secret broker). Hermes is then seeded from it at spawn by
 * `hermes_seed_plan` (src-tauri/src/commands.rs):
 *   - openrouter: NO base URL (the seed's fallback ladder picks OpenRouter when
 *     it holds an OpenRouter key). Setting one would flip the seed to "custom".
 *   - deepseek:   base URL https://api.deepseek.com → first-class "deepseek".
 */
import type { AgentProvider } from './agentProvider';

export type QuickStartProviderId = 'openrouter' | 'deepseek';

/** Default (low-cost, capable) model per provider. Named constants so a bump is
 *  one line.
 *
 *  Both are DeepSeek's "Flash" tier: tool-calling, 1M-token context, and priced at
 *  a small fraction of a frontier model — the cheap default the owner asked for
 *  after Claude proved expensive and confusing for non-technical users. The ids
 *  are provider-specific (the same model has a different slug on each):
 *   - OpenRouter slug `deepseek/deepseek-v4.1-flash` — present, non-expiring and
 *     tool-capable in https://openrouter.ai/api/v1/models (checked 2026-10-15);
 *   - DeepSeek API id `deepseek-flash` — the id used by api-docs.deepseek.com's
 *     quick start and /models example (checked 2026-10-15). The older
 *     `deepseek-v4-flash` is only a still-accepted legacy alias; do not use it.
 *  HOW TO BUMP: open the two URLs above, pick the new id, edit the constant, and
 *  update the check date here. A stronger-but-pricier tier exists for both
 *  (`deepseek/deepseek-v4-pro` / `deepseek-v4-pro`) — users can type it into the
 *  step's Advanced field. The model is editable there, so a retired id never
 *  strands anyone. */
export const QUICK_START_MODEL_OPENROUTER = 'deepseek/deepseek-v4.1-flash';
export const QUICK_START_MODEL_DEEPSEEK = 'deepseek-flash';

export interface QuickStartProvider {
  /** Also the secret-broker backend id (`providerKeySet({ backend })`). */
  id: QuickStartProviderId;
  label: string;
  /** Where the user creates a key — opened with cth.openExternal. */
  keyUrl: string;
  /** Hermes `providerBaseUrls` override; undefined = none (see file header). */
  baseUrl?: string;
  model: string;
  recommended?: boolean;
  /** Soft format hint only — never blocks (providers change key formats). */
  keyPrefix?: string;
}

/** In display order: the recommended provider first. */
export const QUICK_START_PROVIDERS: readonly QuickStartProvider[] = [
  {
    id: 'openrouter',
    label: 'OpenRouter',
    keyUrl: 'https://openrouter.ai/keys',
    model: QUICK_START_MODEL_OPENROUTER,
    recommended: true,
    keyPrefix: 'sk-or-'
  },
  {
    id: 'deepseek',
    label: 'DeepSeek',
    keyUrl: 'https://platform.deepseek.com/api_keys',
    baseUrl: 'https://api.deepseek.com',
    model: QUICK_START_MODEL_DEEPSEEK
  }
];

export const DEFAULT_QUICK_START_PROVIDER: QuickStartProviderId = 'openrouter';

/** The engine quick start always runs: Hermes talks to either provider above. */
export const QUICK_START_ENGINE: AgentProvider = 'hermes';

export function quickStartProvider(id: QuickStartProviderId): QuickStartProvider {
  return QUICK_START_PROVIDERS.find((p) => p.id === id) ?? QUICK_START_PROVIDERS[0];
}

// ─── key validation (format only — no network) ──────────────────────────────

/** A real key is far longer; this only catches a truncated copy / a stray word. */
const MIN_KEY_LENGTH = 16;

export type QuickStartKeyCheck =
  | { ok: true; key: string }
  /** `empty` is NOT an error: the step may be skipped and a key added later. */
  | { ok: false; reason: 'empty' | 'whitespace' | 'short' };

export function validateQuickStartKey(raw: string | undefined): QuickStartKeyCheck {
  const key = (raw ?? '').trim();
  if (!key) return { ok: false, reason: 'empty' };
  if (/\s/.test(key)) return { ok: false, reason: 'whitespace' };
  if (key.length < MIN_KEY_LENGTH) return { ok: false, reason: 'short' };
  return { ok: true, key };
}

/** True when the key does not start the way this provider's keys usually do — a
 *  hint to double-check, never a block. */
export function quickStartKeyLooksOff(provider: QuickStartProviderId, key: string): boolean {
  const prefix = quickStartProvider(provider).keyPrefix;
  return !!prefix && key.trim().length > 0 && !key.trim().startsWith(prefix);
}

/** The model to use: the user's Advanced edit, else the provider default. */
export function quickStartModel(provider: QuickStartProviderId, edited?: string): string {
  return edited?.trim() || quickStartProvider(provider).model;
}

// ─── what Finish writes ─────────────────────────────────────────────────────

/** Who decides Hermes' provider when the user already has their own Hermes
 *  config: `'own'` = their existing setup (no key needed here), `'app'` = the
 *  provider/key chosen in this app overrides it (in the per-agent copy only —
 *  src-tauri/src/hive.rs install_hermes_hooks_seeded_at). Config key `hermesSource`. */
export type HermesSource = 'own' | 'app';

/** What `hermes_user_config_status` answers (read-only; never a key value). */
export interface HermesUserConfigStatus {
  exists?: boolean;
  provider?: string | null;
  model?: string | null;
  hasKeys?: boolean;
  path?: string;
}

export interface HermesChoice {
  /** Offer the "which Hermes setup?" choice at all. */
  show: boolean;
  provider?: string;
  hasKeys: boolean;
  /** Preselected: the user's own setup when it already holds a key (it very
   *  likely works), else this app's key. */
  defaultSource: HermesSource;
}

/** The choice only exists when the user HAS a Hermes config with a provider of
 *  its own; anything else (no status, no file, no provider) is today's flow. */
export function hermesChoiceFor(status: HermesUserConfigStatus | null | undefined): HermesChoice {
  const provider = typeof status?.provider === 'string' ? status.provider.trim() : '';
  const show = status?.exists === true && provider.length > 0;
  const hasKeys = status?.hasKeys === true;
  return { show, provider: show ? provider : undefined, hasKeys, defaultSource: hasKeys ? 'own' : 'app' };
}

export interface QuickStartConfigPatch {
  godProvider: AgentProvider;
  /** `null` clears it (the user's own Hermes provider must not be handed a model
   *  slug picked for another provider). */
  godModel?: string | null;
  providerBaseUrls?: Partial<Record<AgentProvider, string>>;
  providerDefaultModels?: Partial<Record<AgentProvider, string>>;
  hermesSource?: HermesSource;
}

export interface QuickStartExistingConfig {
  providerBaseUrls?: Partial<Record<AgentProvider, string>>;
  providerDefaultModels?: Partial<Record<AgentProvider, string>>;
}

/** The config patch for a quick start. With a key: Hermes + the model (as the
 *  orchestrator's model AND the per-engine default, so workers hired later start
 *  on it too) + the base URL the provider needs. Without one (step skipped):
 *  ONLY the engine — a model or endpoint with no key behind it would send Hermes
 *  to a provider the user never set up; the agent shows its "needs setup" hint
 *  until a key is added in Settings. Existing per-engine maps are merged, never
 *  replaced (the patch writes whole objects). */
export function buildQuickStartPatch(
  input: { provider: QuickStartProviderId; model?: string; hasKey: boolean; hermesSource?: HermesSource },
  existing: QuickStartExistingConfig = {}
): QuickStartConfigPatch {
  // "Use my existing Hermes setup": engine only, and NO model / endpoint of ours —
  // the user's own provider decides all of it.
  if (input.hermesSource === 'own') {
    return { godProvider: QUICK_START_ENGINE, godModel: null, hermesSource: 'own' };
  }
  if (!input.hasKey) return { godProvider: QUICK_START_ENGINE };
  const choice = quickStartProvider(input.provider);
  const model = quickStartModel(input.provider, input.model);
  // Drop any previous hermes endpoint first: an OpenRouter choice must leave NONE.
  const urls = { ...(existing.providerBaseUrls ?? {}) };
  delete urls.hermes;
  if (choice.baseUrl) urls.hermes = choice.baseUrl;
  return {
    godProvider: QUICK_START_ENGINE,
    godModel: model,
    providerBaseUrls: urls,
    providerDefaultModels: { ...(existing.providerDefaultModels ?? {}), hermes: model },
    // A key pasted here is an explicit choice: it must beat whatever Hermes
    // config the user may have (now or installed later).
    hermesSource: 'app'
  };
}

// ─── bridge-touching helpers (the bridge is injected) ───────────────────────

export interface QuickStartBridge {
  providerKeySet(req: { backend: string; key: string }): Promise<{ ok?: boolean; error?: string } | undefined>;
  providerKeyValidate?(provider: string, key: string): Promise<{ code?: string } | undefined>;
}

export type QuickStartKeyVerdict = 'valid' | 'rejected' | 'unreachable';

/** Ask the backend to check the key with one lightweight authenticated GET.
 *  Anything but a clear yes/no — no command, a throw, an odd reply — is
 *  'unreachable' (inconclusive): the caller shows a friendly retry and lets the
 *  user continue, because the check must never be the thing that blocks setup. */
export async function checkQuickStartKey(
  bridge: QuickStartBridge,
  provider: QuickStartProviderId,
  key: string
): Promise<QuickStartKeyVerdict> {
  try {
    const r = await bridge.providerKeyValidate?.(provider, key);
    return r?.code === 'valid' || r?.code === 'rejected' ? r.code : 'unreachable';
  } catch {
    return 'unreachable';
  }
}

export type QuickStartSaveResult =
  | { ok: true; patch: QuickStartConfigPatch }
  | { ok: false; error: string };

/** Finish-time write: store the key (when there is one) through the Settings
 *  path, then hand back the config patch for the wizard to persist. The key is
 *  stored FIRST — if the secret store refuses it, nothing else changes and the
 *  caller can show the error. */
export async function saveQuickStart(
  bridge: QuickStartBridge,
  input: { provider: QuickStartProviderId; key: string; model?: string; hermesSource?: HermesSource },
  existing: QuickStartExistingConfig = {}
): Promise<QuickStartSaveResult> {
  // The user's own Hermes setup is the source: nothing to validate or store.
  if (input.hermesSource === 'own') {
    return { ok: true, patch: buildQuickStartPatch({ provider: input.provider, hasKey: false, hermesSource: 'own' }, existing) };
  }
  const check = validateQuickStartKey(input.key);
  if (!check.ok) {
    if (check.reason === 'empty') {
      return { ok: true, patch: buildQuickStartPatch({ provider: input.provider, hasKey: false }, existing) };
    }
    return { ok: false, error: check.reason };
  }
  try {
    const r = await bridge.providerKeySet({ backend: input.provider, key: check.key });
    if (!r?.ok) return { ok: false, error: r?.error ?? 'rejected' };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
  return {
    ok: true,
    patch: buildQuickStartPatch({ provider: input.provider, model: input.model, hasKey: true }, existing)
  };
}
