/**
 * Hermes-first default + first-run "quick start".
 *
 * SAFETY: every credential write here goes to a FakeBridge `providerKeySet` stub
 * — nothing in this file can reach the OS credential store, and the keys used are
 * obvious placeholders.
 */
import { describe, it, expect, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { OnboardingWizard } from '@/components/OnboardingWizard';
import { installFakeBridge } from './fakeBridge';
import {
  AGENT_PROVIDER_PRESETS, NEW_INSTALL_ENGINE, providerPreset, resolveGodProvider
} from '@/shared/agentProvider';
import { defaultAgentProvider, modelProvidersForAgent, onboardingEngineChoices } from '@/store/config';
import { toolCatalog } from '@/shared/toolCatalog';
import {
  QUICK_START_MODEL_DEEPSEEK, QUICK_START_MODEL_OPENROUTER, QUICK_START_PROVIDERS,
  buildQuickStartPatch, checkQuickStartKey, quickStartKeyLooksOff, saveQuickStart,
  validateQuickStartKey
} from '@/shared/quickStart';
import en from '@/i18n/locales/en.json';
import id from '@/i18n/locales/id.json';
import ar from '@/i18n/locales/ar.json';
import zh from '@/i18n/locales/zh-CN.json';

const KEY = 'sk-or-v1-placeholder-0123456789';
const DS_KEY = 'sk-placeholder-deepseek-0123456789';

describe('engine order: Hermes first, then Claude', () => {
  it('Hermes is first and Claude second in the preset array', () => {
    const ids = AGENT_PROVIDER_PRESETS.map((p) => p.id);
    expect(ids.slice(0, 2)).toEqual(['hermes', 'claude']);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toHaveLength(15);
    expect(ids[ids.length - 1]).toBe('custom');
  });

  it('every list derived from the array leads with Hermes', () => {
    expect(onboardingEngineChoices().eligible[0].id).toBe('hermes');
    expect(onboardingEngineChoices().eligible[1].id).toBe('claude');
    expect(modelProvidersForAgent()[0].id).toBe('hermes');
    expect(modelProvidersForAgent(true)[0].id).toBe('hermes');
    expect(toolCatalog().filter((t) => t.kind === 'engine')[0].id).toBe('engine:hermes');
  });

  it('an unknown provider id still resolves to Claude, not to the first preset', () => {
    expect(providerPreset('zzz' as never).id).toBe('claude');
  });
});

describe('legacy configs are never silently switched off Claude', () => {
  const legacy = { onboardingComplete: true, defaultCommand: 'claude' };

  it('an onboarded config with no godProvider still resolves to Claude', () => {
    expect(resolveGodProvider({ ...legacy, godProvider: undefined })).toBe('claude');
    expect(resolveGodProvider(null)).toBe('claude');
    expect(defaultAgentProvider({ defaultCommand: 'claude', godProvider: undefined })).toBe('claude');
    // The new-install default is a different constant, never the runtime fallback.
    expect(NEW_INSTALL_ENGINE).toBe('hermes');
  });

  it('useHive still falls back to claude for a config with no godProvider', () => {
    const src = readFileSync('src/hooks/useHive.ts', 'utf8');
    expect(src).toMatch(/config\.godProvider \?\? 'claude'/);
  });

  it('a fresh technical onboarding writes hermes explicitly', async () => {
    const updateConfig = vi.fn(async (p: Record<string, unknown>) => ({ ...p }));
    installFakeBridge({
      toolsStatus: async () => [],
      ensureHarnessHome: async () => ({ ok: true }),
      updateConfig
    });
    render(<OnboardingWizard onComplete={() => {}} />);
    await walkTechnicalToFinish();
    fireEvent.click(screen.getByRole('button', { name: /^finish$/i }));
    await waitFor(() => expect(updateConfig).toHaveBeenCalled());
    const patch = updateConfig.mock.calls[0][0] as Record<string, unknown>;
    expect(patch.godProvider).toBe('hermes');
    // No Claude model id rides along with a Hermes orchestrator.
    expect(patch.godModel ?? '').not.toMatch(/claude/i);
  });
});

describe('quick start: pure helpers', () => {
  it('validates the key format (empty is a skip, not an error)', () => {
    expect(validateQuickStartKey('')).toEqual({ ok: false, reason: 'empty' });
    expect(validateQuickStartKey('   ')).toEqual({ ok: false, reason: 'empty' });
    expect(validateQuickStartKey('abc')).toEqual({ ok: false, reason: 'short' });
    expect(validateQuickStartKey('sk-or-v1 with spaces inside it')).toEqual({ ok: false, reason: 'whitespace' });
    expect(validateQuickStartKey(`  ${KEY}\n`)).toEqual({ ok: true, key: KEY });
  });

  it('the prefix check is only a hint', () => {
    expect(quickStartKeyLooksOff('openrouter', KEY)).toBe(false);
    expect(quickStartKeyLooksOff('openrouter', 'sk-other-0123456789abcdef')).toBe(true);
    expect(quickStartKeyLooksOff('deepseek', 'anything-0123456789abcdef')).toBe(false);
    expect(quickStartKeyLooksOff('openrouter', '')).toBe(false);
  });

  it('OpenRouter is the recommended first provider, DeepSeek second, with the key pages', () => {
    expect(QUICK_START_PROVIDERS.map((p) => p.id)).toEqual(['openrouter', 'deepseek']);
    expect(QUICK_START_PROVIDERS[0].recommended).toBe(true);
    expect(QUICK_START_PROVIDERS[0].keyUrl).toBe('https://openrouter.ai/keys');
    expect(QUICK_START_PROVIDERS[1].keyUrl).toBe('https://platform.deepseek.com/api_keys');
    expect(QUICK_START_PROVIDERS.every((p) => p.model && !/claude/i.test(p.model))).toBe(true);
  });

  it('openrouter patch: hermes + model, and NO hermes base URL (it would flip the seed to "custom")', () => {
    const patch = buildQuickStartPatch(
      { provider: 'openrouter', hasKey: true },
      { providerBaseUrls: { hermes: 'https://old.example', qwen: 'http://q' }, providerDefaultModels: { codex: 'gpt-x' } }
    );
    expect(patch.godProvider).toBe('hermes');
    expect(patch.godModel).toBe(QUICK_START_MODEL_OPENROUTER);
    expect(patch.providerBaseUrls).toEqual({ qwen: 'http://q' });
    expect(patch.providerDefaultModels).toEqual({ codex: 'gpt-x', hermes: QUICK_START_MODEL_OPENROUTER });
  });

  it('deepseek patch: the DeepSeek base URL (selects the first-class seed) and its own model id', () => {
    const patch = buildQuickStartPatch({ provider: 'deepseek', hasKey: true });
    expect(patch.godModel).toBe(QUICK_START_MODEL_DEEPSEEK);
    expect(patch.providerBaseUrls).toEqual({ hermes: 'https://api.deepseek.com' });
    expect(patch.providerDefaultModels).toEqual({ hermes: QUICK_START_MODEL_DEEPSEEK });
  });

  it('an edited model wins; a blank edit falls back to the default', () => {
    expect(buildQuickStartPatch({ provider: 'openrouter', model: ' deepseek/deepseek-v4-pro ', hasKey: true }).godModel)
      .toBe('deepseek/deepseek-v4-pro');
    expect(buildQuickStartPatch({ provider: 'openrouter', model: '  ', hasKey: true }).godModel)
      .toBe(QUICK_START_MODEL_OPENROUTER);
  });

  it('skipping the key writes the engine only — no model/endpoint with nothing behind it', () => {
    expect(buildQuickStartPatch({ provider: 'deepseek', hasKey: false })).toEqual({ godProvider: 'hermes' });
  });
});

describe('quick start: what gets stored (fake bridge only)', () => {
  it('stores the key through providerKeySet with the provider as the backend, then returns the patch', async () => {
    const providerKeySet = vi.fn(async () => ({ ok: true }));
    const r = await saveQuickStart({ providerKeySet }, { provider: 'openrouter', key: ` ${KEY} ` });
    expect(providerKeySet).toHaveBeenCalledExactlyOnceWith({ backend: 'openrouter', key: KEY });
    expect(r.ok && r.patch.godProvider).toBe('hermes');
    // The secret is never part of the config patch.
    expect(JSON.stringify(r)).not.toContain(KEY);
  });

  it('never calls the secret store when there is no key, and refuses a malformed one', async () => {
    const providerKeySet = vi.fn(async () => ({ ok: true }));
    const skipped = await saveQuickStart({ providerKeySet }, { provider: 'openrouter', key: '' });
    expect(skipped.ok && skipped.patch).toEqual({ godProvider: 'hermes' });
    const bad = await saveQuickStart({ providerKeySet }, { provider: 'openrouter', key: 'short' });
    expect(bad).toEqual({ ok: false, error: 'short' });
    expect(providerKeySet).not.toHaveBeenCalled();
  });

  it('a refused or throwing secret store changes nothing', async () => {
    const refused = await saveQuickStart(
      { providerKeySet: async () => ({ ok: false, error: 'keyring locked' }) },
      { provider: 'deepseek', key: DS_KEY }
    );
    expect(refused).toEqual({ ok: false, error: 'keyring locked' });
    const threw = await saveQuickStart(
      { providerKeySet: async () => { throw new Error('boom'); } },
      { provider: 'deepseek', key: DS_KEY }
    );
    expect(threw).toEqual({ ok: false, error: 'boom' });
  });

  it('the key check only trusts a clear yes/no', async () => {
    const ask = (reply: unknown) => checkQuickStartKey(
      { providerKeySet: vi.fn(), providerKeyValidate: async () => reply as never }, 'openrouter', KEY
    );
    expect(await ask({ ok: true, code: 'valid' })).toBe('valid');
    expect(await ask({ ok: false, code: 'rejected' })).toBe('rejected');
    expect(await ask({ ok: false, code: 'unreachable' })).toBe('unreachable');
    expect(await ask(undefined)).toBe('unreachable');
    expect(await checkQuickStartKey({ providerKeySet: vi.fn() }, 'openrouter', KEY)).toBe('unreachable');
    expect(await checkQuickStartKey(
      { providerKeySet: vi.fn(), providerKeyValidate: async () => { throw new Error('x'); } }, 'openrouter', KEY
    )).toBe('unreachable');
  });
});

// ─── the wizard ─────────────────────────────────────────────────────────────

function bridgeFor(over: Record<string, unknown> = {}) {
  const updateConfig = vi.fn(async (p: Record<string, unknown>) => ({ ...p }));
  const providerKeySet = vi.fn(async (_r: { backend: string; key: string }) => ({ ok: true }));
  const providerKeyValidate = vi.fn(async (_p: string, _k: string) => ({ ok: true, code: 'valid' }));
  const openExternal = vi.fn(async (_url: string) => true);
  const bridge = installFakeBridge({
    toolsStatus: async () => [],
    ensureHarnessHome: async () => ({ ok: true }),
    updateConfig, providerKeySet, providerKeyValidate, openExternal,
    ...over
  });
  return { bridge, updateConfig, providerKeySet, providerKeyValidate, openExternal };
}

const click = (name: RegExp) => fireEvent.click(screen.getByRole('button', { name }));

/** persona → welcome → home, leaving the wizard on the step after "home". */
function toAfterHome(persona: RegExp) {
  fireEvent.click(screen.getByText(persona));
  click(/^next$/i);
  click(/set it up/i);
  click(/^next$/i); // home → connect / orchestrator
}

async function walkTechnicalToFinish() {
  toAfterHome(/I'M TECHNICAL/i);
  click(/^next$/i); // orchestrator → repos
  click(/^next$/i); // repos → permissions
}

const keyField = () => screen.getByPlaceholderText('Paste your key here') as HTMLInputElement;

describe('onboarding: non-technical quick start', () => {
  it('replaces the engine/model steps with ONE "Connect your AI" step', async () => {
    bridgeFor();
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M NON-TECHNICAL/i);

    expect(screen.getByText(/CONNECT YOUR AI/)).toBeTruthy();
    expect(screen.getByText(/What is an API key/i, { exact: false })).toBeTruthy();
    // No engine list, no orchestrator model picker.
    expect(screen.queryByText(/YOUR CLONE'S ENGINE|YOUR CLONE$/)).toBeNull();
    expect(document.querySelectorAll('input[name="godProvider"]').length).toBe(0);
    // OpenRouter first and recommended, DeepSeek second.
    const radios = [...document.querySelectorAll('input[name="quickStartProvider"]')] as HTMLInputElement[];
    expect(radios.map((r) => r.value)).toEqual(['openrouter', 'deepseek']);
    expect(radios[0].checked).toBe(true);
    expect(screen.getByText('RECOMMENDED')).toBeTruthy();
    // Honest, price-free cost note and a masked field.
    expect(screen.getByText(/pay as you go at the provider/i)).toBeTruthy();
    expect(keyField().type).toBe('password');
    // The CLI way out is a link, not a step of its own.
    expect(screen.getByText(/I already use Claude \/ Codex \/ another CLI/)).toBeTruthy();
  });

  it('the key-page buttons open the provider pages externally', () => {
    const { openExternal } = bridgeFor();
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M NON-TECHNICAL/i);
    const open = screen.getAllByRole('button', { name: /open the sign-up \/ key page/i });
    expect(open).toHaveLength(2);
    fireEvent.click(open[0]);
    fireEvent.click(open[1]);
    expect(openExternal.mock.calls.map((c) => c[0])).toEqual([
      'https://openrouter.ai/keys', 'https://platform.deepseek.com/api_keys'
    ]);
  });

  it('OpenRouter: validates, stores the key via providerKeySet, writes hermes + the model', async () => {
    const { updateConfig, providerKeySet, providerKeyValidate } = bridgeFor();
    const onComplete = vi.fn();
    render(<OnboardingWizard onComplete={onComplete} />);
    toAfterHome(/I'M NON-TECHNICAL/i);

    fireEvent.change(keyField(), { target: { value: ` ${KEY} ` } });
    click(/^next$/i);
    await waitFor(() => expect(screen.getByText(/YOUR PROJECTS/)).toBeTruthy());
    expect(providerKeyValidate).toHaveBeenCalledExactlyOnceWith('openrouter', KEY);
    // Validation alone stores nothing.
    expect(providerKeySet).not.toHaveBeenCalled();

    click(/^next$/i); // repos → permissions
    click(/^finish$/i);
    await waitFor(() => expect(onComplete).toHaveBeenCalledTimes(1));

    expect(providerKeySet).toHaveBeenCalledExactlyOnceWith({ backend: 'openrouter', key: KEY });
    const patch = updateConfig.mock.calls[0][0] as Record<string, unknown>;
    expect(patch).toMatchObject({
      onboardingComplete: true,
      audience: 'non-technical',
      uiMode: 'simple',
      godProvider: 'hermes',
      godModel: QUICK_START_MODEL_OPENROUTER,
      providerDefaultModels: { hermes: QUICK_START_MODEL_OPENROUTER }
    });
    expect((patch.providerBaseUrls as Record<string, string>).hermes).toBeUndefined();
    // The key itself never lands in config.json.
    expect(JSON.stringify(patch)).not.toContain(KEY);
  });

  it('DeepSeek: base URL + its own model id; the Advanced model edit is honoured', async () => {
    const { updateConfig, providerKeySet, providerKeyValidate } = bridgeFor();
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M NON-TECHNICAL/i);

    fireEvent.click(document.querySelector('input[value="deepseek"]')!);
    fireEvent.change(keyField(), { target: { value: DS_KEY } });
    // Collapsed by default, but always in the DOM: edit the model.
    fireEvent.change(screen.getByLabelText('Model'), { target: { value: 'deepseek-v4-pro' } });
    click(/^next$/i);
    await waitFor(() => expect(providerKeyValidate).toHaveBeenCalledWith('deepseek', DS_KEY));
    await waitFor(() => expect(screen.getByText(/YOUR PROJECTS/)).toBeTruthy());
    click(/^next$/i);
    click(/^finish$/i);
    await waitFor(() => expect(updateConfig).toHaveBeenCalled());

    expect(providerKeySet).toHaveBeenCalledExactlyOnceWith({ backend: 'deepseek', key: DS_KEY });
    expect(updateConfig.mock.calls[0][0]).toMatchObject({
      godProvider: 'hermes',
      godModel: 'deepseek-v4-pro',
      providerBaseUrls: { hermes: 'https://api.deepseek.com' },
      providerDefaultModels: { hermes: 'deepseek-v4-pro' }
    });
  });

  it('skipping the key still finishes: engine only, nothing sent to the secret store', async () => {
    const { updateConfig, providerKeySet, providerKeyValidate } = bridgeFor();
    const onComplete = vi.fn();
    render(<OnboardingWizard onComplete={onComplete} />);
    toAfterHome(/I'M NON-TECHNICAL/i);

    click(/^next$/i); // empty field = skip
    click(/^next$/i); // repos → permissions
    click(/^finish$/i);
    await waitFor(() => expect(onComplete).toHaveBeenCalledTimes(1));

    expect(providerKeyValidate).not.toHaveBeenCalled();
    expect(providerKeySet).not.toHaveBeenCalled();
    const patch = updateConfig.mock.calls[0][0] as Record<string, unknown>;
    expect(patch.godProvider).toBe('hermes');
    expect(patch.godModel).toBeUndefined();
    expect(patch.providerBaseUrls).toBeUndefined();
    expect(patch.onboardingComplete).toBe(true);
  });

  it('a malformed key is stopped on the step with a friendly message', () => {
    const { providerKeyValidate } = bridgeFor();
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M NON-TECHNICAL/i);
    fireEvent.change(keyField(), { target: { value: 'tooshort' } });
    expect(screen.getAllByText(/too short to be a full key/i).length).toBeGreaterThan(0);
    click(/^next$/i);
    expect(screen.getByText(/CONNECT YOUR AI/)).toBeTruthy();
    expect(providerKeyValidate).not.toHaveBeenCalled();
  });

  it('a rejected key shows a retry message once, then Next continues anyway', async () => {
    bridgeFor({ providerKeyValidate: vi.fn(async () => ({ ok: false, code: 'rejected' })) });
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M NON-TECHNICAL/i);
    fireEvent.change(keyField(), { target: { value: KEY } });
    click(/^next$/i);
    await waitFor(() => expect(screen.getByText(/OpenRouter did not accept that key/i)).toBeTruthy());
    expect(screen.getByText(/CONNECT YOUR AI/)).toBeTruthy();
    click(/^next$/i); // the second press continues
    await waitFor(() => expect(screen.queryByText(/CONNECT YOUR AI/)).toBeNull());
    expect(screen.getByText(/YOUR PROJECTS/)).toBeTruthy();
  });

  it('an unreachable provider is not a bad key: same friendly retry, still continuable', async () => {
    bridgeFor({ providerKeyValidate: vi.fn(async () => ({ ok: false, code: 'unreachable' })) });
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M NON-TECHNICAL/i);
    fireEvent.change(keyField(), { target: { value: KEY } });
    click(/^next$/i);
    await waitFor(() => expect(screen.getByText(/Could not reach OpenRouter/i)).toBeTruthy());
    click(/^next$/i);
    await waitFor(() => expect(screen.queryByText(/CONNECT YOUR AI/)).toBeNull());
  });

  it('a refused secret store keeps the user on the key step with the reason (nothing is written)', async () => {
    const { updateConfig } = bridgeFor({
      providerKeySet: vi.fn(async () => ({ ok: false, error: 'keyring locked' }))
    });
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M NON-TECHNICAL/i);
    fireEvent.change(keyField(), { target: { value: KEY } });
    click(/^next$/i);
    await waitFor(() => expect(screen.getByText(/YOUR PROJECTS/)).toBeTruthy());
    click(/^next$/i);
    click(/^finish$/i);
    await waitFor(() => expect(screen.getByText(/keyring locked/i)).toBeTruthy());
    expect(updateConfig).not.toHaveBeenCalled();
    expect(screen.getByText(/CONNECT YOUR AI/)).toBeTruthy();
  });

  it('"I already use Claude / Codex / another CLI" opens the normal engine step, Hermes first', () => {
    bridgeFor();
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M NON-TECHNICAL/i);
    fireEvent.click(screen.getByText(/I already use Claude \/ Codex \/ another CLI/));
    expect(document.querySelectorAll('input[name="quickStartProvider"]').length).toBe(0);
    const engines = [...document.querySelectorAll('input[name="godProvider"]')] as HTMLInputElement[];
    expect(engines.length).toBeGreaterThan(2);
    expect(engines[0].value).toBe('hermes');
    expect(engines[1].value).toBe('claude');
    // …and the one-step path is one click back.
    fireEvent.click(screen.getByText(/Quick start: connect with an API key instead/i));
    expect(screen.getByText(/CONNECT YOUR AI/)).toBeTruthy();
  });
});

describe('onboarding: technical path keeps the full engine step (Hermes-first)', () => {
  it('shows the engine list with Hermes first and preselected when nothing is installed', () => {
    const { providerKeyValidate } = bridgeFor();
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M TECHNICAL/i);
    expect(screen.queryByText(/CONNECT YOUR AI/)).toBeNull();
    const engines = [...document.querySelectorAll('input[name="godProvider"]')] as HTMLInputElement[];
    expect(engines[0].value).toBe('hermes');
    expect(engines[0].checked).toBe(true);
    expect(engines[1].value).toBe('claude');
    expect(providerKeyValidate).not.toHaveBeenCalled();
  });

  it('Claude is described qualitatively as the higher-cost option, with no invented prices', () => {
    bridgeFor();
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M TECHNICAL/i);
    expect(screen.getByText(/Uses your Claude subscription or API credits \(higher cost\)/)).toBeTruthy();
    expect(screen.getByText(/Recommended starting engine/)).toBeTruthy();
    for (const loc of [en, id, ar, zh]) {
      expect(loc.onboarding.providerBlurb.claude).not.toMatch(/\$\s?\d/);
    }
  });

  it('a technical user who has only Claude installed is preselected on Claude (no silent Hermes install)', async () => {
    bridgeFor({
      toolsStatus: async () => [
        { id: 'engine:claude', kind: 'engine', found: true, path: 'C:\\bin\\claude.exe', installCommand: '' },
        { id: 'engine:hermes', kind: 'engine', found: false, path: null, installCommand: 'iex' }
      ]
    });
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M TECHNICAL/i);
    await waitFor(() => {
      const checked = document.querySelector('input[name="godProvider"]:checked') as HTMLInputElement;
      expect(checked.value).toBe('claude');
    });
  });

  it('a technical user can still opt into quick start from the engine step', () => {
    bridgeFor();
    render(<OnboardingWizard onComplete={() => {}} />);
    toAfterHome(/I'M TECHNICAL/i);
    fireEvent.click(screen.getByText(/Quick start: connect with an API key instead/i));
    expect(screen.getByText(/CONNECT YOUR AI/)).toBeTruthy();
  });
});

describe('quickStart.* locale parity', () => {
  const flat = (o: unknown, p = ''): Record<string, string> =>
    Object.entries(o as Record<string, unknown>).reduce<Record<string, string>>((acc, [k, v]) => {
      const q = p ? `${p}.${k}` : k;
      if (v && typeof v === 'object') Object.assign(acc, flat(v, q));
      else acc[q] = String(v);
      return acc;
    }, {});
  const ph = (s: string) => [...s.matchAll(/\{\{\s*(\w+)\s*\}\}/g)].map((m) => m[1]).sort().join(',');
  it('identical key sets and placeholders in all four locales', () => {
    const fe = flat(en.quickStart);
    expect(Object.keys(fe).length).toBeGreaterThan(20);
    for (const [name, loc] of Object.entries({ id, ar, zh })) {
      const f = flat(loc.quickStart);
      expect(Object.keys(f).sort(), name).toEqual(Object.keys(fe).sort());
      for (const k of Object.keys(fe)) {
        expect(f[k], `${name}:${k}`).toBeTruthy();
        expect(ph(f[k]), `${name}:${k} placeholders`).toBe(ph(fe[k]));
      }
    }
  });
});
