/**
 * Onboarding wizard E2E — drives the whole flow with a FakeBridge backend and
 * asserts the contract Finish relies on.
 *
 * Regression this pins ("klik finish gak bisa", d2fb78d): the backend used to
 * drop unknown patch keys, so the merged config came back WITHOUT
 * onboardingComplete and the wizard never closed. The test asserts onComplete
 * receives a config where onboardingComplete === true — the old shape fails.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { OnboardingWizard } from '@/components/OnboardingWizard';
import { installFakeBridge, type FakeBridge } from './fakeBridge';
import type { HarnessConfig } from '@/store/config';

/** Bridge that behaves like the fixed backend: updateConfig returns the
 *  merged config (patch keys survive). */
function happyBridge(): { bridge: FakeBridge; updateConfig: ReturnType<typeof vi.fn> } {
  const updateConfig = vi.fn(async (patch: Record<string, unknown>) => ({
    // get_config merge: typed defaults + the patch's own keys.
    autoMode: true,
    missions: [],
    ...patch
  }));
  const bridge = installFakeBridge({
    toolsStatus: async () => [],
    ensureHarnessHome: async () => ({ ok: true, path: 'C:\\Users\\t\\HarnessAgents' }),
    updateConfig
  });
  return { bridge, updateConfig };
}

/** Click through every step to the permissions screen. */
function walkToFinish() {
  // persona → pick "I'M TECHNICAL"
  fireEvent.click(screen.getByText(/I'M TECHNICAL/i));
  fireEvent.click(screen.getByRole('button', { name: /^next$/i }));
  // welcome → "set it up"
  fireEvent.click(screen.getByRole('button', { name: /set it up/i }));
  // home → next (~/HarnessAgents is pre-suggested)
  fireEvent.click(screen.getByRole('button', { name: /^next$/i }));
  // orchestrator → next (Claude selected by default; probe says unknown → never blocks)
  fireEvent.click(screen.getByRole('button', { name: /^next$/i }));
  // repos → next (optional)
  fireEvent.click(screen.getByRole('button', { name: /^next$/i }));
  // now on permissions — finish visible
  return screen.getByRole('button', { name: /^finish$/i });
}

describe('OnboardingWizard E2E', () => {
  it('walks all steps and Finish calls onComplete with onboardingComplete:true', async () => {
    const { updateConfig } = happyBridge();
    const onComplete = vi.fn();
    render(<OnboardingWizard onComplete={onComplete} />);

    const finish = walkToFinish();
    fireEvent.click(finish);

    await waitFor(() => expect(onComplete).toHaveBeenCalledTimes(1));
    const patch = updateConfig.mock.calls[0][0] as Record<string, unknown>;
    expect(patch.onboardingComplete).toBe(true);
    expect(patch.harnessHome).toBeTruthy();
    expect(patch).toHaveProperty('godProvider');
    // The config the app continues with must carry the flag — this is the
    // exact regression that left the wizard open.
    const next = onComplete.mock.calls[0][0] as HarnessConfig;
    expect(next.onboardingComplete).toBe(true);
  });

  it('a rejected updateConfig shows the error and re-arms the button', async () => {
    const updateConfig = vi.fn(async () => {
      throw new Error('disk full');
    });
    installFakeBridge({
      toolsStatus: async () => [],
      ensureHarnessHome: async () => ({ ok: true, path: 'C:\\h' }),
      updateConfig
    });
    const onComplete = vi.fn();
    render(<OnboardingWizard onComplete={onComplete} />);

    fireEvent.click(walkToFinish());

    // Old behaviour: busy stuck on "saving…" forever. Now: error + button back.
    await waitFor(() => expect(screen.getByText(/disk full/i)).toBeTruthy());
    expect(onComplete).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: /^finish$/i })).toBeTruthy();
  });

  it('ensureHarnessHome failure surfaces its error', async () => {
    installFakeBridge({
      toolsStatus: async () => [],
      ensureHarnessHome: async () => ({ ok: false, error: 'cannot create folder' }),
      updateConfig: vi.fn(async (p) => p)
    });
    const onComplete = vi.fn();
    render(<OnboardingWizard onComplete={onComplete} />);

    fireEvent.click(walkToFinish());
    await waitFor(() => expect(screen.getByText(/cannot create folder/i)).toBeTruthy());
    expect(onComplete).not.toHaveBeenCalled();
  });
});
