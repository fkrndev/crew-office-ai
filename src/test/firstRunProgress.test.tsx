/**
 * FirstRunProgress: every state of the card, driven by a fake event source.
 * Nothing here reaches a backend, the keyring or a real engine.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import i18n from 'i18next';
import { FirstRunProgress } from '@/components/FirstRunProgress';
import { AgentCard } from '@/components/AgentCard';
import { useEngineSetup, startEngineSetupFeed } from '@/store/engineSetup';
import { useStore } from '@/store/store';
import { fakeBridge, installFakeBridge } from './fakeBridge';
import type { EngineSetupBridge, EngineSetupEvent } from '@/shared/engineSetup';
import en from '@/i18n/locales/en.json';

const ev = (over: Partial<EngineSetupEvent> = {}): EngineSetupEvent => ({
  agentId: 'god', engine: 'hermes', phase: 'checking', detail: 'binary',
  ts: Date.now(), startedAt: Date.now(), ...over
});
const push = (e: EngineSetupEvent) => act(() => useEngineSetup.getState().apply(e));
const setMode = (m: 'simple' | 'advanced') => act(() => useStore.getState().setUiMode(m));

beforeEach(() => {
  installFakeBridge();
  useEngineSetup.getState().reset();
  void i18n.changeLanguage('en');
  setMode('simple');
});

describe('FirstRunProgress (simple mode: the main card)', () => {
  it('renders nothing without a setup, and nothing during a quick start', () => {
    const { container } = render(<FirstRunProgress agentId="god" />);
    expect(container.firstChild).toBeNull();
    push(ev({ phase: 'starting', detail: 'launch' }));
    expect(screen.queryByTestId('first-run-progress')).toBeNull();
  });

  it('installing: plain title, step counter, current step, honest hint, indeterminate progress', () => {
    render(<FirstRunProgress agentId="god" />);
    push(ev({ phase: 'checking' }));
    push(ev({ phase: 'installing', detail: 'native' }));
    expect(screen.getByText('Setting up your assistant')).toBeTruthy();
    expect(screen.getByText('Step 2 of 3')).toBeTruthy();
    expect(screen.getByText('Installing your assistant')).toBeTruthy();
    expect(screen.getByText(/The first time can take a few minutes/)).toBeTruthy();

    // a11y: a polite live status, a busy progressbar with a text alternative.
    const status = screen.getByRole('status');
    expect(status.getAttribute('aria-live')).toBe('polite');
    expect(status.getAttribute('aria-busy')).toBe('true');
    const bar = screen.getByRole('progressbar');
    expect(bar.getAttribute('aria-busy')).toBe('true');
    expect(bar.getAttribute('aria-label')).toBe('Setup progress');
    expect(bar.getAttribute('aria-valuetext')).toBe('Step 2 of 3: Installing your assistant');
    // Indeterminate: never a fake percentage.
    expect(bar.getAttribute('aria-valuenow')).toBeNull();
    // No failure button while it is working.
    expect(screen.queryByRole('button', { name: /try again|add your key|open settings/i })).toBeNull();
  });

  it('installer sub-steps refine the sentence; preparing is its own step', () => {
    render(<FirstRunProgress agentId="god" />);
    push(ev({ phase: 'installing', detail: 'native' }));
    push(ev({ phase: 'installing', detail: 'python', line: 'Downloading Python runtime' }));
    expect(screen.getByText('Installing Python (a tool it runs on)')).toBeTruthy();
    push(ev({ phase: 'starting', detail: 'relaunch' }));
    push(ev({ phase: 'preparing', detail: 'preparing-deps' }));
    expect(screen.getByText('Step 3 of 3')).toBeTruthy();
    expect(screen.getByText('Preparing everything it needs')).toBeTruthy();
  });

  it('with no installer step the counter honestly has two steps', () => {
    render(<FirstRunProgress agentId="god" />);
    push(ev({ phase: 'checking' }));
    push(ev({ phase: 'preparing', detail: 'preparing-deps' }));
    expect(screen.getByText('Step 2 of 2')).toBeTruthy();
  });

  it('"Show details" reveals the recent raw lines, outside the live region; "Hide details" folds it', () => {
    render(<FirstRunProgress agentId="god" />);
    push(ev({ phase: 'installing', detail: 'native', line: 'Fetching index' }));
    push(ev({ phase: 'installing', detail: 'native', line: 'Extracting archive' }));
    expect(screen.queryByText('Extracting archive')).toBeNull();
    const toggle = screen.getByRole('button', { name: 'Show details' });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    fireEvent.click(toggle);
    expect(screen.getByRole('button', { name: 'Hide details' }).getAttribute('aria-expanded')).toBe('true');
    const log = screen.getByRole('region', { name: 'Recent setup output' });
    expect(log.textContent).toContain('Fetching index');
    expect(log.textContent).toContain('Extracting archive');
    expect(log.getAttribute('aria-live')).toBe('off');
    expect(screen.getByRole('status').contains(log)).toBe(false);
    fireEvent.click(screen.getByRole('button', { name: 'Hide details' }));
    expect(screen.queryByRole('region', { name: 'Recent setup output' })).toBeNull();
  });

  it('says so when there is nothing to show yet', () => {
    render(<FirstRunProgress agentId="god" />);
    push(ev({ phase: 'installing', detail: 'native' }));
    fireEvent.click(screen.getByRole('button', { name: 'Show details' }));
    expect(screen.getByText('Nothing to show yet.')).toBeTruthy();
  });

  it('admits when it is taking a while', () => {
    render(<FirstRunProgress agentId="god" />);
    push(ev({ phase: 'preparing', detail: 'preparing-deps', startedAt: Date.now() - 5 * 60_000 }));
    expect(screen.getByText(/10 minutes or more/)).toBeTruthy();
  });

  it('clears when the assistant is ready', () => {
    render(<FirstRunProgress agentId="god" />);
    push(ev({ phase: 'installing' }));
    expect(screen.getByTestId('first-run-progress')).toBeTruthy();
    push(ev({ phase: 'ready', detail: 'prompt' }));
    expect(screen.queryByTestId('first-run-progress')).toBeNull();
  });

  it('only shows its own agent', () => {
    render(<FirstRunProgress agentId="god" />);
    push(ev({ agentId: 'worker-1', phase: 'installing' }));
    expect(screen.queryByTestId('first-run-progress')).toBeNull();
  });
});

describe('failures: one clear button each', () => {
  const fail = (reason: string, extra: Partial<EngineSetupEvent> = {}) => {
    push(ev({ phase: 'installing', detail: 'native' }));
    push(ev({ phase: 'failed', reason, ...extra }));
  };
  /** Buttons other than the details disclosure. */
  const actions = () =>
    screen.getAllByRole('button').map((b) => b.textContent ?? '').filter((t) => !/details/i.test(t));

  it('network: says to check the internet; the button re-runs the setup', async () => {
    const engineSetupRetry = vi.fn(async () => true);
    render(<FirstRunProgress agentId="god" bridge={{ engineSetupRetry } as EngineSetupBridge} />);
    fail('network', { detail: 'curl: (6) Could not resolve host' });
    expect(screen.getByText('Setup needs your attention')).toBeTruthy();
    expect(screen.getByText("Can't reach the internet")).toBeTruthy();
    expect(screen.getByText(/Check your internet connection and try again/)).toBeTruthy();
    expect(actions()).toEqual(['Try again']);
    // The progress bar is gone: nothing is "working" any more.
    expect(screen.queryByRole('progressbar')).toBeNull();
    expect(screen.getByRole('status').getAttribute('aria-busy')).toBe('false');

    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(engineSetupRetry).toHaveBeenCalledExactlyOnceWith('god');
    // While it restarts the button is disabled and says so...
    await waitFor(() => expect(screen.getByRole('button', { name: 'Trying again…' })).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'Trying again…' }));
    expect(engineSetupRetry).toHaveBeenCalledTimes(1);
    // ...until the backend's next `checking` event starts a fresh attempt.
    push(ev({ phase: 'checking', startedAt: Date.now() + 1 }));
    expect(screen.queryByText("Can't reach the internet")).toBeNull();
    expect(screen.getByText('Checking your computer')).toBeTruthy();
  });

  it('no key: button = Add your key, which opens the AI connection settings', () => {
    const opened: unknown[] = [];
    const on = (e: Event) => opened.push((e as CustomEvent).detail);
    window.addEventListener('cth:open-settings', on);
    render(<FirstRunProgress agentId="god" />);
    fail('no_key');
    expect(screen.getByText('Your assistant needs a key')).toBeTruthy();
    expect(actions()).toEqual(['Add your key']);
    fireEvent.click(screen.getByRole('button', { name: 'Add your key' }));
    expect(opened).toEqual([{ section: 'AI connection' }]);
    window.removeEventListener('cth:open-settings', on);
  });

  it('installer unavailable: button = Open settings', () => {
    const opened: unknown[] = [];
    const on = (e: Event) => opened.push((e as CustomEvent).detail);
    window.addEventListener('cth:open-settings', on);
    render(<FirstRunProgress agentId="god" />);
    fail('installer_unavailable');
    expect(actions()).toEqual(['Open settings']);
    fireEvent.click(screen.getByRole('button', { name: 'Open settings' }));
    expect(opened).toHaveLength(1);
    window.removeEventListener('cth:open-settings', on);
  });

  it.each(['installer_error', 'engine_missing', 'engine_exit', 'timeout', 'spawn_error'])(
    '%s: a plain-language explanation and exactly one button, Try again',
    (reason) => {
      render(<FirstRunProgress agentId="god" />);
      fail(reason, { detail: 'some raw line' });
      expect(actions()).toEqual(['Try again']);
      const f = (en.firstRun.failure as Record<string, { title: string; body: string }>)[reason];
      expect(screen.getByText(f.title)).toBeTruthy();
      expect(screen.getByText(f.body)).toBeTruthy();
    }
  );

  it('the failing raw line is reachable under Show details', () => {
    render(<FirstRunProgress agentId="god" />);
    fail('installer_error', { detail: 'pip failed: disk full' });
    fireEvent.click(screen.getByRole('button', { name: 'Show details' }));
    expect(screen.getByRole('region', { name: 'Recent setup output' }).textContent).toContain('pip failed: disk full');
  });

  it('a retry the backend cannot do ("no agent") falls back to the app re-booting its orchestrator', async () => {
    const reload = vi.fn();
    const engineSetupRetry = vi.fn(async () => { throw 'no agent god'; });
    render(<FirstRunProgress agentId="god" reload={reload} bridge={{ engineSetupRetry } as EngineSetupBridge} />);
    fail('spawn_error');
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(reload).toHaveBeenCalledTimes(1));
  });

  it('any other retry error is shown, and the button comes back', async () => {
    const engineSetupRetry = vi.fn(async () => { throw new Error('pty busy'); });
    render(<FirstRunProgress agentId="god" bridge={{ engineSetupRetry } as EngineSetupBridge} />);
    fail('engine_exit');
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/pty busy/));
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();
  });
});

describe('FirstRunProgress (advanced mode: one compact line)', () => {
  beforeEach(() => setMode('advanced'));

  it('is a single status line with the step, and no card chrome', () => {
    render(<FirstRunProgress agentId="god" />);
    push(ev({ phase: 'installing', detail: 'native' }));
    const line = screen.getByTestId('first-run-progress');
    expect(line.getAttribute('data-variant')).toBe('compact');
    expect(line.getAttribute('role')).toBe('status');
    expect(line.getAttribute('aria-live')).toBe('polite');
    expect(line.textContent).toBe('Setting up hermes: Installing your assistant (2/3)');
    expect(screen.queryByRole('progressbar')).toBeNull();
    expect(screen.queryByText('Setting up your assistant')).toBeNull();
  });

  it('a failure keeps its one button', () => {
    render(<FirstRunProgress agentId="god" />);
    push(ev({ phase: 'installing' }));
    push(ev({ phase: 'failed', reason: 'network' }));
    const line = screen.getByTestId('first-run-progress');
    expect(line.textContent).toContain("Setup of hermes stopped: Can't reach the internet");
    expect(screen.getAllByRole('button').map((b) => b.textContent)).toEqual(['Try again']);
  });

  it('the variant can be forced either way', () => {
    render(<FirstRunProgress agentId="god" variant="full" />);
    push(ev({ phase: 'installing' }));
    expect(screen.getByTestId('first-run-progress').getAttribute('data-variant')).toBe('full');
  });
});

describe('the agent card line', () => {
  const card = (setup?: { failed: boolean; current: number; total: number }) =>
    render(<AgentCard name="Michael" character={'michael' as never} accent={'lemon' as never} status="idle" project="hive" setup={setup} />);

  it('says "Setting up… 2/3" while the engine installs, and "Setup needs attention" when it fails', () => {
    const { unmount } = card({ failed: false, current: 2, total: 3 });
    expect(screen.getByTestId('agent-card-setup').textContent).toBe('Setting up… 2/3');
    unmount();
    card({ failed: true, current: 2, total: 3 });
    expect(screen.getByTestId('agent-card-setup').textContent).toBe('Setup needs attention');
  });

  it('is untouched when no setup is active', () => {
    card(undefined);
    expect(screen.queryByTestId('agent-card-setup')).toBeNull();
  });
});

describe('end to end with a fake event source', () => {
  it('events from the bridge drive the card from install to ready', async () => {
    const bridge = fakeBridge();
    const off = startEngineSetupFeed(bridge as never);
    render(<FirstRunProgress agentId="god" />);
    act(() => bridge.emit('onEngineSetup', ev({ phase: 'checking' })));
    act(() => bridge.emit('onEngineSetup', ev({ phase: 'installing', detail: 'native' })));
    expect(screen.getByText('Installing your assistant')).toBeTruthy();
    act(() => bridge.emit('onEngineSetup', ev({ phase: 'preparing', detail: 'preparing-deps' })));
    expect(screen.getByText('Preparing everything it needs')).toBeTruthy();
    act(() => bridge.emit('onEngineSetup', ev({ phase: 'ready', detail: 'hook' })));
    expect(screen.queryByTestId('first-run-progress')).toBeNull();
    off();
  });
});
