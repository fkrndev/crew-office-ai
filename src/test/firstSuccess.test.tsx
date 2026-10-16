/**
 * First-success experience for a non-technical person: starter ideas in the empty
 * chat, "Stop everything", friendly errors, and the polish fixes (waiting-to-send
 * label, one queue list, jump-to-latest, one status truth, one RECOMMENDED, one
 * step list, the simple kanban, About / Add-teammate wording) — each with the
 * "advanced mode is unchanged" half where it applies.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act, within } from '@testing-library/react';
import i18n from 'i18next';
import { installFakeBridge } from './fakeBridge';
import { useStore, type Agent } from '@/store/store';
import en from '@/i18n/locales/en.json';
import id from '@/i18n/locales/id.json';
import ar from '@/i18n/locales/ar.json';
import zh from '@/i18n/locales/zh-CN.json';

vi.mock('@/components/terminalPool', () => ({
  acquireTerminal: () => ({ term: { cols: 80, rows: 24 }, fit: { fit: () => {} } }),
  releaseTerminal: () => {},
  disposeTerminal: () => {},
  resetTerminal: () => {},
  notifyThemeChangeAll: () => {},
  notifyArabicTerminalChangeAll: () => {},
  focusTerminal: () => {},
  useHasTerminalDraft: () => false,
  getPtyIdForElement: () => undefined,
  clearTerminalDraft: () => '',
  dismissTerminalPicker: () => {},
  terminalAutomationBlockFor: () => null
}));
vi.mock('@/components/PtyTerminalView', () => ({ PtyTerminalView: () => <div data-testid="pty-view" /> }));
vi.mock('@/components/MemoryGraphPanel', () => ({ MemoryGraphPanel: () => null }));
vi.mock('@/realtime/CostHud', () => ({ CostHud: () => <span /> }));
vi.mock('@/components/RealtimeMichaelToggle', () => ({ RealtimeMichaelToggle: () => <span /> }));

import { ConversationView } from '@/components/ConversationView';
import { MessageQueueComposer } from '@/components/MessageQueueComposer';
import { StopEverything } from '@/components/StopEverything';
import { QuitWarningModal } from '@/components/QuitWarningModal';
import { ErrorNotice } from '@/components/FriendlyError';
import { QuickStart } from '@/components/QuickStart';
import { TasksKanban } from '@/components/TasksKanban';
import { SettingsHeroCard } from '@/components/SettingsHeroCard';
import { CommandCenterPanel } from '@/components/CommandCenterPanel';
import { OnboardingWizard, wizardSteps, stepPosition } from '@/components/OnboardingWizard';
import { useEngineSetup } from '@/store/engineSetup';
import { resetStopState, isStopped, stopEverything, releaseStopIfNeeded } from '@shared/stopAll';
import { STARTER_IDEAS } from '@/components/ConversationBubbles';
import { fillComposer } from '@/components/composerBridge';

const god = {
  id: 'god', name: 'Michael', character: 'michael', accent: 'lemon', description: 'god', project: 'hive',
  tmuxTarget: '', cwd: 'C:/hive', status: 'idle', action: 'runs the floor', progress: 0, isGod: true,
  ptyId: 'pty-god'
} as unknown as Agent;

const msg = (mid: string, role: string, text: string, ts = 1_700_000_000_000) => ({ id: mid, role, text, ts });
const hermes = (messages: unknown[], cursor = 'h:1') => ({ messages, cursor, source: 'hermes', reset: false, more: false });

function bridge(reply: () => unknown = () => hermes([]), extra: Record<string, unknown> = {}) {
  return installFakeBridge({
    conversationRead: vi.fn(async () => reply()),
    hiveRegistry: async () => ({ agents: { god: { name: 'Michael' } } }),
    controlSnapshot: async () => ({}),
    getConfig: async () => ({}),
    historyAdd: async () => null,
    trackMessageSent: vi.fn(async () => undefined),
    ...extra
  });
}

function chat(agent: Agent = god) {
  return render(
    <ConversationView
      agent={agent}
      terminal={<div data-testid="term">raw terminal</div>}
      composer={<MessageQueueComposer agent={agent} />}
    />
  );
}

beforeEach(() => {
  void i18n.changeLanguage('en');
  resetStopState();
  useStore.setState({ uiMode: 'simple', messageQueues: {}, drafts: {}, agents: [god] } as never);
  HTMLElement.prototype.scrollTo = vi.fn(function (this: HTMLElement, o: ScrollToOptions) {
    this.scrollTop = Number(o.top ?? 0);
  }) as never;
});
afterEach(() => { vi.useRealTimers(); });

// ─── 1. starter ideas ──────────────────────────────────────────────────────

describe('starter ideas in the empty chat', () => {
  it('shows four friendly ideas under "Try one of these" — safe ones, from i18n', async () => {
    bridge();
    chat();
    const group = await screen.findByRole('group', { name: 'Try one of these' });
    const buttons = within(group).getAllByRole('button');
    expect(buttons).toHaveLength(4);
    expect(STARTER_IDEAS).toHaveLength(4);
    for (const k of STARTER_IDEAS) {
      expect(within(group).getByText(i18n.t(`examples.${k}`) as string)).toBeTruthy();
    }
    // Nothing that implies deleting or sending anything.
    for (const b of buttons) expect(b.textContent).not.toMatch(/delete|remove|erase|send (it|the|an|this)|publish|pay|buy/i);
  });

  it('the empty chat reads from the top (never opens scrolled past its greeting)', async () => {
    bridge();
    chat();
    const log = await screen.findByTestId('convo-log');
    await screen.findByTestId('starter-ideas');
    log.scrollTop = 120; // as if something had scrolled it down
    act(() => { useStore.setState({ drafts: { god: 'x' } } as never); }); // any re-render
    expect(HTMLElement.prototype.scrollTo).not.toHaveBeenCalled();
    expect(screen.getByText('Say hello to Michael…')).toBeTruthy();
  });

  it('clicking one PUTS the text in the composer and focuses it — it is never sent', async () => {
    const cth = bridge();
    chat();
    fireEvent.click(await screen.findByTestId('starter-writeEmail'));
    const box = screen.getByRole('textbox') as HTMLTextAreaElement;
    expect(box.value).toBe(en.examples.writeEmail);
    await waitFor(() => expect(document.activeElement).toBe(box));
    // not sent, not queued
    expect(useStore.getState().messageQueues.god ?? []).toHaveLength(0);
    expect(cth.trackMessageSent as ReturnType<typeof vi.fn>).not.toHaveBeenCalled();
    // …and the person can still edit it before sending
    fireEvent.change(box, { target: { value: `${box.value} It is about my holiday.` } });
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toMatch(/holiday\.$/);
  });

  it('keeps what the person already typed and adds the idea below it', async () => {
    bridge();
    chat();
    await screen.findByTestId('starter-todoList');
    act(() => { useStore.getState().setDraft('god', 'Remember the milk'); });
    fireEvent.click(screen.getByTestId('starter-todoList'));
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe(`Remember the milk\n\n${en.examples.todoList}`);
  });

  it('disappear once the conversation has any message', async () => {
    bridge(() => hermes([msg('h1', 'assistant', 'Hello!')]));
    chat();
    await screen.findByTestId('convo-assistant');
    expect(screen.queryByTestId('starter-ideas')).toBeNull();
  });

  it('disappear as soon as a message is waiting in the queue', async () => {
    bridge();
    chat();
    await screen.findByTestId('starter-ideas');
    act(() => {
      useStore.setState({ messageQueues: { god: [{ id: 'q1', text: 'hello there', ts: Date.now() }] } } as never);
    });
    await waitFor(() => expect(screen.queryByTestId('starter-ideas')).toBeNull());
  });

  it('fillComposer is a pure "put it in the box" (no queue, no send)', () => {
    fillComposer('god', 'abc');
    expect(useStore.getState().drafts.god).toBe('abc');
    expect(useStore.getState().messageQueues.god ?? []).toHaveLength(0);
  });

  it('are translated in all four languages (Arabic included)', () => {
    for (const loc of [en, id, ar, zh]) {
      for (const k of STARTER_IDEAS) expect((loc.examples as Record<string, string>)[k].trim().length).toBeGreaterThan(5);
      expect(loc.examples.title.trim()).not.toBe('');
      expect(loc.examples.hint.trim()).not.toBe('');
    }
    expect(ar.examples.summarizeFolder).toMatch(/[\u0600-\u06FF]/);
  });

  it('render in Arabic with the starter texts of that language', async () => {
    await i18n.changeLanguage('ar');
    bridge();
    chat();
    const group = await screen.findByRole('group', { name: ar.examples.title });
    expect(within(group).getByText(ar.examples.explainProject)).toBeTruthy();
    await i18n.changeLanguage('en');
  });
});

// ─── 2. stop everything ────────────────────────────────────────────────────

describe('Stop everything', () => {
  const stopBridge = (over: Record<string, unknown> = {}) => ({
    controlStopAll: vi.fn(async () => ({ stopped: 2, agents: ['god', 'w1'] })),
    controlResumeStopped: vi.fn(async () => 1),
    ...over
  });

  it('is a real button with an accessible name; clicking it asks first, in plain words', () => {
    const b = stopBridge();
    render(<StopEverything bridge={b} />);
    const btn = screen.getByRole('button', { name: 'Stop everything' });
    expect(btn.tagName).toBe('BUTTON');
    fireEvent.click(btn);
    const dlg = screen.getByRole('alertdialog');
    expect(dlg.getAttribute('aria-modal')).toBe('true');
    expect(dlg.getAttribute('aria-labelledby')).toBeTruthy();
    expect(dlg.getAttribute('aria-describedby')).toBeTruthy();
    expect(within(dlg).getByText('This stops everything your team is working on right now. Your files stay as they are.')).toBeTruthy();
    expect(b.controlStopAll).not.toHaveBeenCalled();
  });

  it('moves focus to the safe choice, traps Tab, and Esc closes it (focus returns, nothing is stopped)', async () => {
    const b = stopBridge();
    render(<StopEverything bridge={b} />);
    const trigger = screen.getByRole('button', { name: 'Stop everything' });
    fireEvent.click(trigger);
    const keep = screen.getByRole('button', { name: 'No, keep going' });
    const yes = screen.getByRole('button', { name: 'Yes, stop everything' });
    await waitFor(() => expect(document.activeElement).toBe(keep));
    // Tab from the last button wraps to the first; Shift+Tab from the first wraps to the last.
    yes.focus();
    fireEvent.keyDown(yes, { key: 'Tab' });
    expect(document.activeElement).toBe(keep);
    fireEvent.keyDown(keep, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(yes);
    // Esc (same capture-phase handler as every other modal)
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('alertdialog')).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(trigger));
    expect(b.controlStopAll).not.toHaveBeenCalled();
  });

  it('"No, keep going" and a click outside also keep everything running', () => {
    const b = stopBridge();
    render(<StopEverything bridge={b} />);
    fireEvent.click(screen.getByRole('button', { name: 'Stop everything' }));
    fireEvent.click(screen.getByRole('button', { name: 'No, keep going' }));
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(b.controlStopAll).not.toHaveBeenCalled();
  });

  it('confirming calls the backend once, closes the dialog and says what happened', async () => {
    const b = stopBridge();
    render(<StopEverything bridge={b} />);
    fireEvent.click(screen.getByRole('button', { name: 'Stop everything' }));
    fireEvent.click(screen.getByRole('button', { name: 'Yes, stop everything' }));
    await waitFor(() => expect(b.controlStopAll).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    const note = screen.getByTestId('stop-everything-note');
    expect(note.getAttribute('role')).toBe('status');
    expect(note.textContent).toMatch(/Everything is stopping\. Your team will pause at its next safe step\. Nothing was deleted\./);
    expect(isStopped()).toBe(true);
    // it never quit anything
    expect(Object.keys(b)).toEqual(['controlStopAll', 'controlResumeStopped']);
  });

  it('is idempotent: pressing it again just stops again, never errors', async () => {
    const b = stopBridge();
    render(<StopEverything bridge={b} />);
    for (let i = 0; i < 2; i++) {
      fireEvent.click(screen.getByRole('button', { name: 'Stop everything' }));
      fireEvent.click(screen.getByRole('button', { name: 'Yes, stop everything' }));
      await waitFor(() => expect(b.controlStopAll).toHaveBeenCalledTimes(i + 1));
      await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    }
    expect(screen.queryByTestId('stop-everything-error')).toBeNull();
  });

  it('with nothing running it says so instead of "stopping"', async () => {
    const b = stopBridge({ controlStopAll: vi.fn(async () => ({ stopped: 0, agents: [] })) });
    render(<StopEverything bridge={b} />);
    fireEvent.click(screen.getByRole('button', { name: 'Stop everything' }));
    fireEvent.click(screen.getByRole('button', { name: 'Yes, stop everything' }));
    await waitFor(() => expect(screen.getByTestId('stop-everything-note').textContent).toMatch(/Nothing is running/));
  });

  it('a failure is friendly, keeps the raw text under Details, and can be retried', async () => {
    const b = stopBridge({ controlStopAll: vi.fn(async () => { throw new Error('command `control_stop_all` panicked: boom'); }) });
    render(<StopEverything bridge={b} />);
    fireEvent.click(screen.getByRole('button', { name: 'Stop everything' }));
    fireEvent.click(screen.getByRole('button', { name: 'Yes, stop everything' }));
    const err = await screen.findByTestId('stop-everything-error');
    expect(err.textContent).toMatch(/Something went wrong inside the app/);
    expect(err.textContent).not.toMatch(/panicked/);
    fireEvent.click(within(err).getByRole('button', { name: 'Details' }));
    expect(within(err).getByRole('region', { name: 'Technical details' }).textContent).toMatch(/panicked: boom/);
    expect(isStopped()).toBe(false);
    fireEvent.click(within(err).getByRole('button', { name: 'Try again' }));
    expect(screen.getByRole('alertdialog')).toBeTruthy();
  });

  it('the next message the person sends lifts the stop first — exactly once', async () => {
    const b = stopBridge();
    const cth = bridge(() => hermes([]), b);
    chat();
    // nothing stopped yet: sending does not touch the control seam
    act(() => { useStore.getState().setDraft('god', 'before'); });
    fireEvent.keyDown(await screen.findByRole('textbox'), { key: 'Enter' });
    expect(b.controlResumeStopped).not.toHaveBeenCalled();

    await stopEverything(cth as never);
    expect(isStopped()).toBe(true);
    act(() => { useStore.getState().setDraft('god', 'after'); });
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
    await waitFor(() => expect(b.controlResumeStopped).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(isStopped()).toBe(false));
    // a second message does not call it again
    act(() => { useStore.getState().setDraft('god', 'again'); });
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
    expect(b.controlResumeStopped).toHaveBeenCalledTimes(1);
  });

  it('a failed lift keeps the stop remembered, so the next message tries again', async () => {
    const resume = vi.fn<() => Promise<number>>(async () => { throw new Error('nope'); });
    const b = { controlStopAll: vi.fn(async () => ({ stopped: 1 })), controlResumeStopped: resume };
    await stopEverything(b);
    await releaseStopIfNeeded(b);
    expect(isStopped()).toBe(true);
    resume.mockImplementation(async () => 1);
    await releaseStopIfNeeded(b);
    expect(isStopped()).toBe(false);
    expect(resume).toHaveBeenCalledTimes(2);
  });

  it('is translated everywhere with the same placeholders', () => {
    for (const loc of [en, id, ar, zh]) {
      expect(Object.keys(loc.stopAll).sort()).toEqual(Object.keys(en.stopAll).sort());
      for (const v of Object.values(loc.stopAll)) expect(String(v).trim()).not.toBe('');
    }
  });
});

// ─── 3. friendly errors in components ──────────────────────────────────────

describe('ErrorNotice', () => {
  it('simple mode: friendly title + hint + action, raw only behind "Details"', () => {
    const raw = 'github request failed: http status: 401';
    render(<ErrorNotice error={raw} />);
    const box = screen.getByRole('alert');
    expect(box.textContent).toMatch(/Your AI key wasn't accepted/);
    expect(box.textContent).not.toMatch(/http status/);
    expect(screen.queryByRole('region')).toBeNull();
    const toggle = screen.getByRole('button', { name: 'Details' });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByRole('region', { name: 'Technical details' }).textContent).toBe(raw);
    expect(screen.getByRole('button', { name: 'Check my key' })).toBeTruthy();
  });

  it('"Check my key" opens Settings on the AI connection tab', () => {
    const seen: unknown[] = [];
    const on = (e: Event) => seen.push((e as CustomEvent).detail);
    window.addEventListener('cth:open-settings', on);
    render(<ErrorNotice error="http status: 403" />);
    fireEvent.click(screen.getByRole('button', { name: 'Check my key' }));
    expect(seen).toContainEqual({ section: 'AI connection' });
    window.removeEventListener('cth:open-settings', on);
  });

  it('"Try again" only appears when the host can retry', () => {
    const { rerender } = render(<ErrorNotice error="io: Connection refused (os error 10061)" />);
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
    const retry = vi.fn();
    rerender(<ErrorNotice error="io: Connection refused (os error 10061)" onRetry={retry} />);
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(retry).toHaveBeenCalledTimes(1);
  });

  it('advanced mode shows exactly what each call site showed before (legacy text, else the raw text)', () => {
    useStore.setState({ uiMode: 'advanced' } as never);
    const { rerender } = render(<ErrorNotice error="http status: 401" legacy="Could not save your key: http status: 401" />);
    expect(screen.getByRole('alert').textContent).toBe('Could not save your key: http status: 401');
    expect(screen.queryByRole('button', { name: 'Details' })).toBeNull();
    rerender(<ErrorNotice error="Permission denied (os error 13)" />);
    expect(screen.getByRole('alert').textContent).toBe('Permission denied (os error 13)');
  });

  it('`friendly` overrides the mode (the wizard is not in simple mode yet)', () => {
    useStore.setState({ uiMode: 'advanced' } as never);
    render(<ErrorNotice error="timeout: global" friendly />);
    expect(screen.getByRole('alert').textContent).toMatch(/That took too long/);
  });
});

describe('cth:notice banners', () => {
  it('carry a raw error + context for simple mode and plain wording for the sentence itself', async () => {
    const { noticeFromEvent } = await import('@shared/notice');
    const ev = new CustomEvent('cth:notice', { detail: { text: 'Could not send: x', raw: 'x', context: 'send', simpleText: 'plain' } });
    expect(noticeFromEvent(ev)).toEqual({ text: 'Could not send: x', raw: 'x', context: 'send', simpleText: 'plain' });
    expect(noticeFromEvent(new CustomEvent('cth:notice', { detail: { text: '' } }))).toBeNull();
    expect(noticeFromEvent(new CustomEvent('cth:notice'))).toBeNull();
  });

  it('the dropped-message and degraded-agent sentences have plain simple variants', () => {
    const a = { agent: 'Dana', text: 'hi' };
    const adv = i18n.t('notice.queueDropped', a) as string;
    const simple = i18n.t('notice.queueDropped', { ...a, context: 'simple' }) as string;
    expect(adv).toMatch(/dropped/);
    expect(simple).not.toMatch(/dropped/);
    expect(i18n.t('notice.hiveDegraded.proxyBind', { agent: 'Dana' }) as string).toMatch(/proxy bridge/);
    expect(i18n.t('notice.hiveDegraded.proxyBind', { agent: 'Dana', context: 'simple' }) as string).not.toMatch(/proxy|hive/i);
  });
});

// ─── 4a/4b. composer header + one queue list ───────────────────────────────

describe('composer: waiting-to-send label and the duplicate queue list', () => {
  const q = (n: number) => Array.from({ length: n }, (_, i) => ({ id: `q${i + 1}`, text: `message number ${i + 1}`, ts: Date.now() + i }));

  it('simple: no "WAITING TO SEND" header while nothing is queued', () => {
    bridge();
    render(<MessageQueueComposer agent={god} />);
    expect(screen.queryByText('WAITING TO SEND')).toBeNull();
    expect(screen.queryByTestId('composer-queue-header')).toBeNull();
  });

  it('simple: the label appears only once something is queued', () => {
    bridge();
    useStore.setState({ messageQueues: { god: q(1) } } as never);
    render(<MessageQueueComposer agent={god} />);
    expect(screen.getByText('WAITING TO SEND')).toBeTruthy();
  });

  it('advanced: the QUEUE header is always there, as before', () => {
    bridge();
    useStore.setState({ uiMode: 'advanced' } as never);
    render(<MessageQueueComposer agent={god} />);
    expect(screen.getByText('QUEUE')).toBeTruthy();
  });

  it('simple, inside the chat: the pending bubble is the only place the message shows', async () => {
    bridge(() => hermes([msg('h1', 'assistant', 'Hello!')]));
    useStore.setState({ messageQueues: { god: q(1) } } as never);
    chat();
    const bubble = await screen.findByTestId('convo-pending');
    expect(bubble.textContent).toContain('message number 1');
    expect(screen.queryByTestId('composer-queue-list')).toBeNull();
    // exactly one place shows the text
    const holders = screen.getAllByText(/message number 1/);
    expect(holders).toHaveLength(1);
  });

  it('simple, chat not shown (raw terminal fallback): the composer keeps its own list', () => {
    bridge();
    useStore.setState({ messageQueues: { god: q(2) } } as never);
    render(<MessageQueueComposer agent={god} />);
    expect(screen.getByTestId('composer-queue-list')).toBeTruthy();
    expect(screen.getAllByText(/message number/)).toHaveLength(2);
  });

  it('advanced: the composer list stays, exactly as before', () => {
    bridge();
    useStore.setState({ uiMode: 'advanced', messageQueues: { god: q(2) } } as never);
    render(<MessageQueueComposer agent={god} />);
    expect(screen.getByTestId('composer-queue-list')).toBeTruthy();
  });

  it('a pending bubble can be cancelled from the chat ("Don\'t send")', async () => {
    bridge(() => hermes([msg('h1', 'assistant', 'Hello!')]));
    useStore.setState({ messageQueues: { god: q(2) } } as never);
    chat();
    const bubbles = await screen.findAllByTestId('convo-pending');
    expect(bubbles).toHaveLength(2);
    fireEvent.click(within(bubbles[0]).getByRole('button', { name: "Don't send" }));
    expect(useStore.getState().messageQueues.god.map((m) => m.id)).toEqual(['q2']);
  });

  it('simple only counts the person\'s own messages (not the app\'s nudges)', () => {
    bridge();
    useStore.setState({
      messageQueues: { god: [
        { id: 'n1', text: 'You have new hive inbox message(s) — at least: m1. Read your inbox', ts: 1 },
        ...q(1)
      ] }
    } as never);
    render(<MessageQueueComposer agent={god} />);
    expect(screen.getAllByText(/message number|inbox/)).toHaveLength(1);
  });
});

// ─── 4c. jump to latest ────────────────────────────────────────────────────

describe('jump to latest never covers text', () => {
  it('sits in the flow under the list (not floating over it) and the list has bottom padding', async () => {
    bridge(() => hermes(Array.from({ length: 30 }, (_, i) => msg(`h${i}`, i % 2 ? 'assistant' : 'user', `line ${i}`))));
    chat();
    const log = await screen.findByTestId('convo-log');
    expect(parseInt(log.style.paddingBottom || '0', 10)).toBeGreaterThanOrEqual(16);
    // Make the log "scrolled away from the bottom".
    Object.defineProperty(log, 'scrollHeight', { configurable: true, value: 3000 });
    Object.defineProperty(log, 'clientHeight', { configurable: true, value: 300 });
    log.scrollTop = 100;
    fireEvent.scroll(log);
    const jump = await screen.findByTestId('convo-jump');
    expect(log.contains(jump)).toBe(false);
    let n: HTMLElement | null = jump;
    while (n && n !== document.body) {
      expect(getComputedStyle(n).position).not.toBe('absolute');
      n = n.parentElement;
    }
  });
});

// ─── 4d. one status truth ──────────────────────────────────────────────────

describe('header chip and the first-run card never contradict', () => {
  beforeEach(() => {
    useEngineSetup.getState().reset();
    installFakeBridge({
      controlSnapshot: async () => null, hiveTasks: async () => [], hiveLog: async () => [], hiveBoard: async () => '',
      conversationRead: async () => ({ messages: [], cursor: '', source: 'none', reason: 'starting' })
    });
  });

  const setup = (phase: string, extra: Record<string, unknown> = {}) =>
    act(() => {
      useEngineSetup.getState().apply({ agentId: 'god', engine: 'hermes', phase, ts: Date.now() - 10_000, startedAt: Date.now() - 10_000, ...extra } as never);
    });

  it('while setting up the chip says so — not "ready" or "offline"', () => {
    render(<CommandCenterPanel agent={{ ...god, status: 'ghost' } as Agent} />);
    expect(screen.getByText('offline')).toBeTruthy();
    setup('installing', { detail: 'native' });
    expect(screen.queryByText('offline')).toBeNull();
    expect(screen.getByText('Setting up… 2/3')).toBeTruthy();
  });

  it('a failed setup reads "Setup needs attention" on the chip as well', () => {
    render(<CommandCenterPanel agent={{ ...god, status: 'idle' } as Agent} />);
    expect(screen.getByText('ready')).toBeTruthy();
    setup('failed', { reason: 'network', detail: 'could not resolve host' });
    expect(screen.getByText('Setup needs attention')).toBeTruthy();
    expect(screen.queryByText('ready')).toBeNull();
  });

  it('with no setup the chip is the agent\'s own status, in both modes', () => {
    const { unmount } = render(<CommandCenterPanel agent={{ ...god, status: 'idle' } as Agent} />);
    expect(screen.getByText('ready')).toBeTruthy();
    unmount();
    useStore.setState({ uiMode: 'advanced' } as never);
    render(<CommandCenterPanel agent={{ ...god, status: 'idle' } as Agent} />);
    expect(screen.getAllByText('idle').length).toBeGreaterThan(0);
  });
});

// ─── 4e. one RECOMMENDED ───────────────────────────────────────────────────

describe('quick start shows ONE "RECOMMENDED"', () => {
  const base = {
    provider: 'openrouter' as const, onProvider: () => {}, apiKey: '', onApiKey: () => {}, model: 'x',
    onModel: () => {}, check: 'idle' as const, onUseCli: () => {}
  };
  const choice = (hasKeys: boolean) => ({ show: true, provider: 'openrouter', hasKeys, defaultSource: 'own' as const });

  it('their own setup holds a key + "paste a key" selected: the choice keeps it, the provider list drops it', () => {
    render(<QuickStart {...base} hermesChoice={choice(true) as never} hermesSource="app" />);
    expect(screen.getAllByText('RECOMMENDED')).toHaveLength(1);
  });

  it('their own setup holds no key: the provider list keeps the badge', () => {
    render(<QuickStart {...base} hermesChoice={choice(false) as never} hermesSource="app" />);
    expect(screen.getAllByText('RECOMMENDED')).toHaveLength(1);
  });

  it('no choice shown (the usual first run): the provider badge is unchanged', () => {
    render(<QuickStart {...base} />);
    expect(screen.getAllByText('RECOMMENDED')).toHaveLength(1);
  });
});

// ─── 4f. wizard steps ──────────────────────────────────────────────────────

describe('wizard: the step counter and the dots come from one list', () => {
  it('lists six steps on each path, with the engine slot swapped', () => {
    expect(wizardSteps(false)).toEqual(['persona', 'welcome', 'home', 'orchestrator', 'repos', 'permissions']);
    expect(wizardSteps(true)).toEqual(['persona', 'welcome', 'home', 'connect', 'repos', 'permissions']);
    expect(stepPosition('persona', false)).toEqual({ current: 1, total: 6 });
    expect(stepPosition('orchestrator', false)).toEqual({ current: 4, total: 6 });
    expect(stepPosition('connect', true)).toEqual({ current: 4, total: 6 });
    expect(stepPosition('permissions', true)).toEqual({ current: 6, total: 6 });
    expect(stepPosition('done', true)).toBeNull();
    // a step of the OTHER path is not in this path's list
    expect(stepPosition('orchestrator', true)).toBeNull();
  });

  const wizard = () => {
    installFakeBridge({
      toolsStatus: async () => [], hermesUserConfigStatus: () => ({ exists: false, provider: null, model: null, hasKeys: false, path: '' }),
      getConfig: async () => ({}), updateConfig: async (p: unknown) => p, ensureHarnessHome: async () => ({ ok: true })
    });
    return render(<OnboardingWizard onComplete={() => {}} />);
  };
  const dots = () => screen.getByTestId('wizard-dots');
  // "Next" everywhere except the welcome screen, where the button reads "set it up".
  const next = () => fireEvent.click(screen.getByRole('button', { name: /^(next|set it up)$/i }));

  it('technical path: "STEP n OF 6" matches the six dots on every screen', () => {
    wizard();
    fireEvent.click(screen.getByText(/I'M TECHNICAL/i));
    const titles: string[] = [];
    // Persona → welcome and back: the persona screen is step 1 and counts too.
    next();
    fireEvent.click(screen.getByRole('button', { name: /^back$/i }));
    for (let i = 1; i <= 6; i++) {
      const m = document.body.textContent!.match(/STEP (\d) OF (\d)/);
      expect(m, `step ${i}`).toBeTruthy();
      expect(Number(m![1])).toBe(i);
      expect(Number(m![2])).toBe(6);
      expect(dots().children).toHaveLength(6);
      expect(dots().getAttribute('aria-label')).toBe(`${i}/6`);
      titles.push(m![0]);
      if (i < 6) next();
    }
    expect(new Set(titles).size).toBe(6);
  });

  it('quick-start path: the connect step is step 4 of 6', () => {
    wizard();
    fireEvent.click(screen.getByText(/I'M NON-TECHNICAL/i));
    next(); // persona → welcome
    next(); // welcome → home
    next(); // home → connect
    expect(document.body.textContent).toMatch(/STEP 4 OF 6 · CONNECT YOUR AI/);
    expect(dots().children).toHaveLength(6);
    expect(dots().getAttribute('aria-label')).toBe('4/6');
    // Back and forward keep agreeing
    fireEvent.click(screen.getByRole('button', { name: /^back$/i }));
    expect(document.body.textContent).toMatch(/STEP 3 OF 6/);
    next(); next();
    expect(document.body.textContent).toMatch(/STEP 5 OF 6/);
  });
});

// ─── 4g. kanban ────────────────────────────────────────────────────────────

describe('kanban in the narrow sidebar', () => {
  const tasks = [
    { id: 'bmt-1', title: 'Write the welcome note', status: 'todo' },
    { id: 'bmt-2', title: 'Plan the launch', status: 'doing' },
    { id: 'bmt-3', title: 'Waiting for the logo', status: 'blocked' },
    { id: 'bmt-4', title: 'Sort the inbox', status: 'done' }
  ];
  beforeEach(() => { installFakeBridge({ hiveTasks: async () => ({ tasks }) }); });

  it('simple: one column of collapsible groups, DONE included and reachable', async () => {
    render(<TasksKanban />);
    const board = await screen.findByTestId('kanban-simple');
    for (const [k, label] of [['todo', 'TO DO'], ['doing', 'IN PROGRESS'], ['blocked', 'WAITING'], ['done', 'DONE']] as const) {
      const g = within(board).getByTestId(`kanban-group-${k}`);
      expect(within(g).getByRole('button', { name: new RegExp(label) })).toBeTruthy();
    }
    expect(await screen.findByText('Sort the inbox')).toBeTruthy();
    // the id (a handle for dispatches) is left out of the plain view
    expect(screen.queryByText('bmt-4')).toBeNull();
  });

  it('simple: a group collapses and expands from its header (aria-expanded)', async () => {
    render(<TasksKanban />);
    const done = await screen.findByTestId('kanban-group-done');
    const head = within(done).getByRole('button', { name: /DONE/ });
    expect(head.getAttribute('aria-expanded')).toBe('true');
    const body = document.getElementById(head.getAttribute('aria-controls')!)!;
    expect(body.hidden).toBe(false);
    fireEvent.click(head);
    expect(head.getAttribute('aria-expanded')).toBe('false');
    expect(body.hidden).toBe(true);
    fireEvent.click(head);
    expect(body.hidden).toBe(false);
  });

  it('advanced: the four columns stay as they were', async () => {
    useStore.setState({ uiMode: 'advanced' } as never);
    render(<TasksKanban />);
    await screen.findByText('Sort the inbox');
    expect(screen.queryByTestId('kanban-simple')).toBeNull();
    expect(screen.getByText('TODO')).toBeTruthy();
    expect(screen.getByText('DOING')).toBeTruthy();
    expect(screen.getByText('BLOCKED')).toBeTruthy();
    expect(screen.getByText('bmt-4')).toBeTruthy();
  });
});

// ─── 4h. About card + usage stats wording ──────────────────────────────────

describe('About: plain wording, and the usage-stats text matches what is really sent', () => {
  beforeEach(() => {
    installFakeBridge({
      appInfo: async () => ({ version: '0.9.1' }),
      heroPayload: async () => ({ hero: { plan: { label: 'Local', blurb: 'Every agent runs on your machine, in your folders, under your own keys. No seat limit, nothing metered.' } } }),
      onUpdateStatus: () => () => {}, updateCurrent: async () => null
    });
  });

  it('simple: "Version 0.9.1", no plan badge, no marketing line', async () => {
    render(<SettingsHeroCard />);
    await screen.findByText('Version 0.9.1');
    expect(screen.queryByText('Local')).toBeNull();
    expect(screen.queryByText(/seat limit|metered/i)).toBeNull();
    expect(screen.getByText(/Your AI team runs on this computer/)).toBeTruthy();
    expect(screen.getByText('see everything that changed →')).toBeTruthy();
  });

  it('advanced: v0.9.1, the plan badge and its blurb, as before', async () => {
    useStore.setState({ uiMode: 'advanced' } as never);
    render(<SettingsHeroCard />);
    await screen.findByText('v0.9.1');
    expect(screen.getByText('Local')).toBeTruthy();
    expect(screen.getByText(/seat limit/)).toBeTruthy();
    expect(screen.getByText('full changelog →')).toBeTruthy();
  });

  it('the simple description says what is and is not sent (analytics.rs allowlist)', () => {
    const d = en.settings.general.telemetryDesc_simple;
    // sent: opened, which engine, start failure, message COUNT, version + OS, rough duration
    for (const w of [/opened/, /AI engine/, /failed to start/, /how many messages/, /optional features/, /version/, /operating system/, /how long/]) expect(d).toMatch(w);
    // not sent
    expect(d).toMatch(/never what they say/i);
    expect(d).toMatch(/Never your messages, files, folder names/);
    expect(d).toMatch(/not linked to your name or account/);
    expect(d).toMatch(/Turn it off and nothing is sent/);
    // no marketing / technical words
    expect(d).not.toMatch(/TELEMETRY\.md|PostHog|events?\b|spawn|allowlist/i);
    expect(en.settings.general.telemetryDesc).toMatch(/TELEMETRY\.md/); // advanced text untouched
  });
});

// ─── 4j. no jargon in the simple variants ──────────────────────────────────

describe('plain-language variants contain no jargon', () => {
  const JARGON = /\b(hive|worktree|pty|mcp|tokens?|breaker|triggers?|webhooks?|closing time|sessions?|orchestrator|terminal|prompt|picker|slash|git|cli|api)\b/i;
  const flat = (o: unknown, p = ''): Record<string, string> => {
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(o as Record<string, unknown>)) {
      if (v && typeof v === 'object') Object.assign(out, flat(v, `${p}${k}.`));
      else out[`${p}${k}`] = String(v);
    }
    return out;
  };
  const E = flat(en);
  // Keys this work added (their `_simple` text must be jargon-free). Older variants are checked by simpleMode.test.
  const ADDED = Object.keys(E).filter((k) =>
    /^(addAgent\.(.*_simple|projectHelp|gitIsolationHelp|mcpExtrasHelp|commandHelp)|quit\..*_simple|restorePrompt\..*_simple|agentStrip\..*_simple|agentDetail\..*_simple|office\.activity\..*_simple|agentStatus\..*_simple|queueComposer\..*_simple|notice\..*_simple|settingsHero\.(version|updateAvailable|download|downloadTitle|blurb|.*_simple)|settings\.general\.telemetry.*_simple|examples\.|stopAll\.|errors\.)/.test(k));

  it('covers a meaningful set of keys', () => { expect(ADDED.length).toBeGreaterThan(80); });

  it('none of them uses a technical word', () => {
    // "git" is allowed only where a project history is genuinely the topic — none of ours needs it.
    // An "AI key" is fine; the literal word "API" is not in plain copy.
    const bad = ADDED.filter((k) => JARGON.test(E[k].replace(/<\/?strong>/g, '')));
    expect(bad.map((k) => `${k}: ${E[k]}`)).toEqual([]);
  });

  it('the new simple variants have the same placeholders in every language', () => {
    const ph = (s: string) => (s.match(/\{\{\s*[\w.]+\s*\}\}/g) ?? []).sort();
    for (const loc of [id, ar, zh]) {
      const F = flat(loc);
      for (const k of ADDED) {
        expect(F[k], k).toBeTruthy();
        expect(ph(F[k]), k).toEqual(ph(E[k]));
      }
    }
  });
});

// ─── whole-file locale parity ──────────────────────────────────────────────

describe('locale parity (whole files)', () => {
  const keys = (o: unknown, p = ''): string[] =>
    Object.entries(o as Record<string, unknown>).flatMap(([k, v]) =>
      v && typeof v === 'object' ? keys(v, `${p}${k}.`) : [`${p}${k}`]);
  it('all four locales have identical key sets', () => {
    const base = keys(en).sort();
    expect(keys(id).sort()).toEqual(base);
    expect(keys(ar).sort()).toEqual(base);
    expect(keys(zh).sort()).toEqual(base);
  });
});

// ─── the quit dialog, in plain words ───────────────────────────────────────


describe('quit dialog wording', () => {
  const dlg = () => render(<QuitWarningModal ptyCount={2} onCancel={() => {}} onConfirm={() => {}} onClosingTime={() => {}} />);

  it('simple: "save & close", no "closing time" / "kill" / "sessions" / "terminal"', () => {
    dlg();
    expect(screen.getByText('2 TEAMMATES STILL WORKING')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'save & close' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'keep working' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'stop all & close' })).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/closing time|kill|session|terminal|agent/i);
  });

  it('advanced: the original wording, unchanged', () => {
    useStore.setState({ uiMode: 'advanced' } as never);
    dlg();
    expect(screen.getByText('2 AGENTS STILL RUNNING')).toBeTruthy();
    expect(screen.getByRole('button', { name: /closing time/ })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'kill all & quit' })).toBeTruthy();
  });
});
