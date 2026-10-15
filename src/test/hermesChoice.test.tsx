/**
 * "Use my existing Hermes setup" vs "use the key I paste here": the choice UI in
 * the quick start and in Settings → AI connection, and exactly what each one
 * writes. SAFETY: every credential write goes to a FakeBridge `providerKeySet`
 * stub — nothing here can reach the OS credential store (whose service name is
 * shared with the user's real app); the keys are obvious placeholders.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import i18n from 'i18next';
import { OnboardingWizard } from '@/components/OnboardingWizard';
import { SimpleAiConnection } from '@/components/SimpleAiConnection';
import { installFakeBridge } from './fakeBridge';
import { buildQuickStartPatch, hermesChoiceFor, saveQuickStart } from '@/shared/quickStart';
import type { HarnessConfig } from '@/store/config';

const KEY = 'sk-or-v1-placeholder-0123456789';

const OWN_ANTHROPIC = { exists: true, provider: 'anthropic', model: 'claude-x', hasKeys: true, path: 'C:/tmp/none' };

function bridgeFor(status: unknown, over: Record<string, unknown> = {}) {
  const updateConfig = vi.fn(async (p: Record<string, unknown>) => ({ ...p }));
  const providerKeySet = vi.fn(async (_r: { backend: string; key: string }) => ({ ok: true }));
  const providerKeyValidate = vi.fn(async (_p: string, _k: string) => ({ ok: true, code: 'valid' }));
  installFakeBridge({
    toolsStatus: async () => [],
    ensureHarnessHome: async () => ({ ok: true }),
    hermesUserConfigStatus: async () => status,
    providerKeyHas: async () => false,
    updateConfig, providerKeySet, providerKeyValidate,
    ...over
  });
  return { updateConfig, providerKeySet, providerKeyValidate };
}

const click = (name: RegExp) => fireEvent.click(screen.getByRole('button', { name }));
function toConnectStep() {
  fireEvent.click(screen.getByText(/I'M NON-TECHNICAL/i));
  click(/^next$/i);
  click(/set it up/i);
  click(/^next$/i);
}
const ownRadio = () => screen.getByRole('radio', { name: /Use my existing Hermes setup/i }) as HTMLInputElement;
const appRadio = () => screen.getByRole('radio', { name: /Use the key I paste here instead/i }) as HTMLInputElement;
const keyField = () => screen.queryByPlaceholderText('Paste your key here') as HTMLInputElement | null;

beforeEach(() => { void i18n.changeLanguage('en'); });

describe('hermesChoiceFor', () => {
  it('only offers the choice when the user has a Hermes config WITH a provider', () => {
    expect(hermesChoiceFor(undefined).show).toBe(false);
    expect(hermesChoiceFor(null).show).toBe(false);
    expect(hermesChoiceFor({ exists: false, provider: null }).show).toBe(false);
    expect(hermesChoiceFor({ exists: true, provider: null, hasKeys: true }).show).toBe(false);
    expect(hermesChoiceFor({ exists: true, provider: '  ' }).show).toBe(false);
    expect(hermesChoiceFor(OWN_ANTHROPIC)).toEqual({ show: true, provider: 'anthropic', hasKeys: true, defaultSource: 'own' });
  });

  it('preselects their own setup only when it already holds a key', () => {
    expect(hermesChoiceFor({ exists: true, provider: 'x', hasKeys: true }).defaultSource).toBe('own');
    expect(hermesChoiceFor({ exists: true, provider: 'x', hasKeys: false }).defaultSource).toBe('app');
  });
});

describe('what the quick start writes', () => {
  it('own: engine only, hermesSource own, the model cleared, no key stored, no endpoint or model of ours', async () => {
    const providerKeySet = vi.fn(async () => ({ ok: true }));
    const r = await saveQuickStart({ providerKeySet }, { provider: 'openrouter', key: KEY, hermesSource: 'own' });
    expect(providerKeySet).not.toHaveBeenCalled();
    expect(r.ok && r.patch).toEqual({ godProvider: 'hermes', godModel: null, hermesSource: 'own' });
  });

  it('a pasted key is an explicit choice: hermesSource app (and the usual model / endpoint)', () => {
    const p = buildQuickStartPatch({ provider: 'deepseek', hasKey: true });
    expect(p.hermesSource).toBe('app');
    expect(p.providerBaseUrls).toEqual({ hermes: 'https://api.deepseek.com' });
    // Skipping stays the bare engine patch (older behaviour, untouched).
    expect(buildQuickStartPatch({ provider: 'deepseek', hasKey: false })).toEqual({ godProvider: 'hermes' });
  });
});

describe('onboarding: the user already has a Hermes setup', () => {
  it('shows the explicit choice in plain words, preselecting their own setup when it has a key', async () => {
    bridgeFor(OWN_ANTHROPIC);
    render(<OnboardingWizard onComplete={() => {}} />);
    toConnectStep();
    await waitFor(() => expect(screen.getByTestId('hermes-source-choice')).toBeTruthy());
    expect(screen.getByRole('radio', { name: /Use my existing Hermes setup \(provider: anthropic\)/ })).toBeTruthy();
    expect(screen.getByText(/No key needed here\. Recommended if it already works\./)).toBeTruthy();
    expect(ownRadio().checked).toBe(true);
    expect(appRadio().checked).toBe(false);
    // "Own" needs nothing pasted: the key field and provider list are out of the way.
    expect(keyField()).toBeNull();
    expect(document.querySelectorAll('input[name="quickStartProvider"]').length).toBe(0);
  });

  it('own: Next skips the key check; Finish stores NO key and writes hermesSource own', async () => {
    const { updateConfig, providerKeySet, providerKeyValidate } = bridgeFor(OWN_ANTHROPIC);
    const onComplete = vi.fn();
    render(<OnboardingWizard onComplete={onComplete} />);
    toConnectStep();
    await waitFor(() => expect(ownRadio()).toBeTruthy());
    click(/^next$/i);
    await waitFor(() => expect(screen.getByText(/YOUR PROJECTS/)).toBeTruthy());
    click(/^next$/i);
    click(/^finish$/i);
    await waitFor(() => expect(onComplete).toHaveBeenCalledTimes(1));
    expect(providerKeyValidate).not.toHaveBeenCalled();
    expect(providerKeySet).not.toHaveBeenCalled();
    const patch = updateConfig.mock.calls[0][0] as Record<string, unknown>;
    expect(patch).toMatchObject({ onboardingComplete: true, godProvider: 'hermes', hermesSource: 'own', godModel: null });
    expect(patch.providerBaseUrls).toBeUndefined();
    expect(patch.providerDefaultModels).toBeUndefined();
  });

  it('app: the key field comes back; the pasted key is validated, stored and wins (hermesSource app)', async () => {
    const { updateConfig, providerKeySet, providerKeyValidate } = bridgeFor(OWN_ANTHROPIC);
    const onComplete = vi.fn();
    render(<OnboardingWizard onComplete={onComplete} />);
    toConnectStep();
    await waitFor(() => expect(appRadio()).toBeTruthy());
    fireEvent.click(appRadio());
    expect(keyField()).not.toBeNull();
    fireEvent.change(keyField()!, { target: { value: KEY } });
    click(/^next$/i);
    await waitFor(() => expect(screen.getByText(/YOUR PROJECTS/)).toBeTruthy());
    expect(providerKeyValidate).toHaveBeenCalledExactlyOnceWith('openrouter', KEY);
    click(/^next$/i);
    click(/^finish$/i);
    await waitFor(() => expect(onComplete).toHaveBeenCalledTimes(1));
    expect(providerKeySet).toHaveBeenCalledExactlyOnceWith({ backend: 'openrouter', key: KEY });
    const patch = updateConfig.mock.calls[0][0] as Record<string, unknown>;
    expect(patch).toMatchObject({ godProvider: 'hermes', hermesSource: 'app' });
    expect(JSON.stringify(patch)).not.toContain(KEY);
  });

  it('their config has a provider but no key: it is NOT preselected, and the note says why', async () => {
    bridgeFor({ ...OWN_ANTHROPIC, hasKeys: false });
    render(<OnboardingWizard onComplete={() => {}} />);
    toConnectStep();
    await waitFor(() => expect(appRadio()).toBeTruthy());
    expect(appRadio().checked).toBe(true);
    expect(screen.getByText(/we didn't find a key in it/i)).toBeTruthy();
    expect(keyField()).not.toBeNull();
  });

  it.each([
    ['no Hermes config at all', { exists: false, provider: null, model: null, hasKeys: false, path: '' }],
    ['a config without a provider', { exists: true, provider: null, model: null, hasKeys: false, path: '' }],
    ['no answer (older backend)', undefined]
  ])('%s: today\'s step, no choice', async (_n, status) => {
    bridgeFor(status);
    render(<OnboardingWizard onComplete={() => {}} />);
    toConnectStep();
    await waitFor(() => expect(keyField()).not.toBeNull());
    expect(screen.queryByTestId('hermes-source-choice')).toBeNull();
  });
});

describe('Settings → AI connection', () => {
  const config = (over: Partial<HarnessConfig> = {}) => ({ godProvider: 'hermes', ...over }) as HarnessConfig;
  const renderConn = (cfg: HarnessConfig, engine: 'hermes' | 'claude' = 'hermes') =>
    render(<SimpleAiConnection config={cfg} engine={engine} engineChanged={false} onPickEngine={() => {}} />);

  it('shows the choice (own preselected when nothing was stored) and hides the key box for "own"', async () => {
    bridgeFor(OWN_ANTHROPIC);
    renderConn(config());
    await waitFor(() => expect(screen.getByTestId('hermes-source-choice')).toBeTruthy());
    expect(ownRadio().checked).toBe(true);
    expect(screen.getByText(/Your assistant will use your existing Hermes setup/)).toBeTruthy();
    expect(screen.queryByText('Your key')).toBeNull();
  });

  it('switching to "app" saves hermesSource at once and brings the key field back; back to "own" clears the model', async () => {
    const { updateConfig, providerKeySet } = bridgeFor(OWN_ANTHROPIC);
    renderConn(config({ hermesSource: 'own' }));
    await waitFor(() => expect(appRadio()).toBeTruthy());
    fireEvent.click(appRadio());
    await waitFor(() => expect(updateConfig).toHaveBeenCalledWith({ hermesSource: 'app' }));
    await waitFor(() => expect(screen.getByText(/Saved\. This applies the next time your assistant starts/)).toBeTruthy());
    expect(screen.getByText('Your key')).toBeTruthy();
    expect(ownRadio().checked).toBe(false);

    fireEvent.click(ownRadio());
    await waitFor(() => expect(updateConfig).toHaveBeenLastCalledWith({ hermesSource: 'own', godModel: null }));
    expect(screen.queryByText('Your key')).toBeNull();
    // Nothing in this flow touches a key.
    expect(providerKeySet).not.toHaveBeenCalled();
  });

  it('reflects a stored "app" choice', async () => {
    bridgeFor(OWN_ANTHROPIC);
    renderConn(config({ hermesSource: 'app' }));
    await waitFor(() => expect(appRadio()).toBeTruthy());
    expect(appRadio().checked).toBe(true);
    expect(screen.getByText('Your key')).toBeTruthy();
  });

  it('reports a save that failed', async () => {
    bridgeFor(OWN_ANTHROPIC, { updateConfig: async () => { throw new Error('disk full'); } });
    renderConn(config());
    await waitFor(() => expect(appRadio()).toBeTruthy());
    fireEvent.click(appRadio());
    await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/Couldn't save that choice/));
  });

  it('no choice for another engine, or without a Hermes config of their own', async () => {
    bridgeFor(OWN_ANTHROPIC);
    const { unmount } = renderConn(config({ godProvider: 'claude' }), 'claude');
    await Promise.resolve();
    expect(screen.queryByTestId('hermes-source-choice')).toBeNull();
    unmount();
    bridgeFor({ exists: false, provider: null, model: null, hasKeys: false, path: '' });
    renderConn(config());
    await Promise.resolve();
    expect(screen.queryByTestId('hermes-source-choice')).toBeNull();
    expect(screen.getByText('Your key')).toBeTruthy();
  });
});
