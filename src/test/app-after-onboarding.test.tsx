/**
 * Post-onboarding render E2E — the "blank page after Finish" regression.
 *
 * Renders <App/> exactly as it stands right after the wizard: config arrives
 * via getConfig, onboardingComplete is true, hiveOpened comes from the wizard
 * path (we simulate the real sequence by mounting App fresh with a completed
 * config — the wizard path and cold-launch path share the same render tree).
 *
 * Heavy leaf components (pixi canvas, xterm, monaco, the codemirror IDE) are
 * mocked to inert divs: jsdom has no WebGL. Everything AROUND them — the
 * shell, panels, hooks, store wiring — renders for real, so a crash in the
 * post-onboarding tree fails here instead of in the user's hands.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, waitFor } from '@testing-library/react';
import { installFakeBridge } from './fakeBridge';

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
  getPtyIdForElement: () => undefined
}));
vi.mock('@/components/FullscreenTerminal', () => ({
  FullscreenTerminal: () => null
}));
vi.mock('@/ide/IdePanel', () => ({ IdePanel: () => null }));
vi.mock('@/components/AddAgentModal', () => ({ AddAgentModal: () => null }));
vi.mock('@/components/SettingsModal', () => ({ SettingsModal: () => null }));
vi.mock('@/components/QuitWarningModal', () => ({ QuitWarningModal: () => null }));
vi.mock('@/components/TaskDetailOverlay', () => ({ TaskDetailOverlay: () => null }));
vi.mock('@/components/MemoryPanel', () => ({ MemoryPanel: () => null }));
vi.mock('@/components/AgentDetailPanel', () => ({ AgentDetailPanel: () => null }));

// The freeflow hold-to-talk hook binds global listeners — inert in tests.
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

describe('App after onboarding completes', () => {
  beforeEach(() => {
    // The wizard's onComplete sets hiveOpened=true; a cold boot reaches the
    // same state via the one-shot skip flag a hive switch leaves behind.
    window.localStorage.setItem('cth.skipHivePickerOnce', '1');
    installFakeBridge({
      getConfig: async () => ({ ...COMPLETED_CONFIG }),
      realtimeHasOpenAiKey: async () => false,
      listPtys: async () => [],
      hiveRegistry: async () => [],
      hiveTasks: async () => [],
      hiveBoard: async () => '',
      drainPendingHires: async () => []
    });
  });

  it('renders the main shell — never a blank page', async () => {
    const { container } = render(<App />);
    // The shell mounts synchronously once config arrives.
    await waitFor(() => {
      expect(container.querySelector('[data-testid="office-floor"]')).toBeTruthy();
    });
    // And the app is not stuck on a blank div.
    expect(container.innerHTML.length).toBeGreaterThan(500);
  });

  it('a completed config reaching the tree does not blank the floor', async () => {
    const { container } = render(<App />);
    await waitFor(() => {
      expect(container.querySelector('[data-testid="office-floor"]')).toBeTruthy();
    });
  });
});
