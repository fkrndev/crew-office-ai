/**
 * God boot failure paths — the "CLOCKING IN loader forever" regression.
 *
 * MichaelBooting renders while godStatus === 'booting'; the only exits are
 * 'ready' (spawn ok / already live) and 'failed'. Before the fix, a spawnPty
 * that THREW (or never resolved) left godStatus pinned at 'booting' — the
 * loader spun eternally with zero feedback. These tests pin every exit.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, waitFor } from '@testing-library/react';
import { installFakeBridge } from './fakeBridge';
import { useStore } from '@/store/store';

vi.mock('@/scene/office/OfficeFloor', () => ({
  OfficeFloor: () => <div data-testid="office-floor" />
}));
vi.mock('@/components/terminalPool', () => ({
  acquireTerminal: () => {},
  releaseTerminal: () => {},
  notifyThemeChangeAll: () => {},
  focusTerminal: () => {},
  suspendTerminal: () => {},
  isTerminalLive: () => false,
  detachTerminal: () => {},
  resizeTerminal: () => {},
  writeToTerminal: () => {},
  setLinkProvider: () => {},
  getPtyIdForElement: () => undefined,
  resetTerminal: () => {}
}));
vi.mock('@/components/FullscreenTerminal', () => ({ FullscreenTerminal: () => null }));
vi.mock('@/ide/IdePanel', () => ({ IdePanel: () => null }));
vi.mock('@/components/AddAgentModal', () => ({ AddAgentModal: () => null }));
vi.mock('@/components/SettingsModal', () => ({ SettingsModal: () => null }));
vi.mock('@/components/QuitWarningModal', () => ({ QuitWarningModal: () => null }));
vi.mock('@/components/TaskDetailOverlay', () => ({ TaskDetailOverlay: () => null }));
vi.mock('@/components/MemoryPanel', () => ({ MemoryPanel: () => null }));
vi.mock('@/components/AgentDetailPanel', () => ({ AgentDetailPanel: () => null }));
vi.mock('@/freeflow/holdOption', () => ({ useHoldOptionToTalk: () => {} }));

import { App } from '@/App';

const COMPLETED_CONFIG = {
  onboardingComplete: true,
  audience: 'technical',
  harnessHome: 'C:\\Users\\t\\HarnessAgents',
  registeredRepos: [],
  autoMode: true,
  godProvider: 'claude',
  godModel: 'claude-opus-4-8',
  telemetryEnabled: true,
  missions: [],
  notifications: false,
  webhooks: [],
  secretEnvKeys: []
};

function baseBridge(overrides: Record<string, unknown> = {}) {
  installFakeBridge({
    getConfig: async () => ({ ...COMPLETED_CONFIG }),
    realtimeHasOpenAiKey: async () => false,
    listPtys: async () => [],
    hiveRegistry: async () => [],
    hiveTasks: async () => [],
    hiveBoard: async () => '',
    drainPendingHires: async () => null,
    ...overrides
  });
}

describe('god boot failure paths', () => {
  beforeEach(() => {
    window.localStorage.setItem('cth.skipHivePickerOnce', '1');
    useStore.setState({ godStatus: 'booting', agents: [], archivedAgents: [] });
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it('spawnPty REJECTING leaves booting → failed, never a stuck loader', async () => {
    baseBridge({ spawnPty: async () => { throw new Error('bridge dead'); } });
    render(<App />);
    await waitFor(() => {
      expect(useStore.getState().godStatus).toBe('failed');
    }, { timeout: 8000 });
  });

  it('spawnPty returning ok:false also reaches failed (not stuck)', async () => {
    baseBridge({ spawnPty: async () => ({ ok: false, error: 'no binary' }) });
    render(<App />);
    await waitFor(() => {
      expect(useStore.getState().godStatus).toBe('failed');
    }, { timeout: 8000 });
  });

  it('a spawnPty that NEVER resolves trips the 30s watchdog → failed', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    // Pending forever — the dead-bridge case.
    baseBridge({ spawnPty: () => new Promise(() => {}) });
    render(<App />);
    // The boot effect waits 1.2s before spawning; the watchdog needs 30s of
    // wall clock after mount. Advance both.
    await vi.advanceTimersByTimeAsync(1500);
    await vi.advanceTimersByTimeAsync(30_000);
    await vi.waitFor(() => {
      expect(useStore.getState().godStatus).toBe('failed');
    });
  });

  it('god PTY already live → ready without spawning (resume path)', async () => {
    const spawnPty = vi.fn();
    baseBridge({
      listPtys: async () => [{ id: 'pty-god' }],
      spawnPty
    });
    render(<App />);
    await waitFor(() => {
      expect(useStore.getState().godStatus).toBe('ready');
    }, { timeout: 10000 });
    expect(spawnPty).not.toHaveBeenCalled();
  }, 15000);

  it('onboarding not complete → boot effect never fires', async () => {
    const spawnPty = vi.fn();
    baseBridge({
      getConfig: async () => ({ ...COMPLETED_CONFIG, onboardingComplete: false }),
      spawnPty
    });
    render(<App />);
    // Give the boot timeout a chance to fire if it wrongly did.
    await new Promise((r) => setTimeout(r, 2000));
    expect(spawnPty).not.toHaveBeenCalled();
  });
});
