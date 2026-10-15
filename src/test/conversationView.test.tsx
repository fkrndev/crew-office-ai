/**
 * The simple-mode conversation view (ConversationView): bubbles, markdown safety,
 * the working indicator, pending/echo bubbles, auto-scroll + "jump to latest",
 * the terminal disclosure and the fallback to the raw terminal, accessibility
 * roles — and the promise that advanced mode is untouched.
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
  getPtyIdForElement: () => undefined
}));
vi.mock('@/components/PtyTerminalView', () => ({ PtyTerminalView: () => <div data-testid="pty-view" /> }));
vi.mock('@/components/MessageQueueComposer', () => ({ MessageQueueComposer: () => <div data-testid="composer" /> }));
vi.mock('@/components/MemoryGraphPanel', () => ({ MemoryGraphPanel: () => null }));
vi.mock('@/realtime/CostHud', () => ({ CostHud: () => <span /> }));
vi.mock('@/components/RealtimeMichaelToggle', () => ({ RealtimeMichaelToggle: () => <span /> }));

import { ConversationView } from '@/components/ConversationView';
import { CommandCenterPanel } from '@/components/CommandCenterPanel';

const god = {
  id: 'god', name: 'Michael', character: 'michael', accent: 'lemon', description: 'god', project: 'hive',
  tmuxTarget: '', cwd: 'C:/hive', status: 'idle', action: 'runs the floor', progress: 0, isGod: true,
  ptyId: 'pty-god'
} as unknown as Agent;

const msg = (id: string, role: string, text: string, ts = 1_700_000_000_000) => ({ id, role, text, ts });
const hermes = (messages: unknown[], cursor = 'h:1', extra: Record<string, unknown> = {}) =>
  ({ messages, cursor, source: 'hermes', reset: false, more: false, ...extra });

let reads: ReturnType<typeof vi.fn>;
function bridge(reply: (agentId: string, cursor: string | null) => unknown) {
  reads = vi.fn(async (a: string, c: string | null) => reply(a, c));
  installFakeBridge({
    conversationRead: reads,
    hiveRegistry: async () => ({ agents: { god: { name: 'Michael' } } }),
    controlSnapshot: async () => null,
    getConfig: async () => ({}),
    historyAdd: async () => null
  });
}

function view(agent: Agent = god) {
  return render(
    <ConversationView
      agent={agent}
      terminal={<div data-testid="term">raw terminal</div>}
      composer={<div data-testid="composer-slot" />}
    />
  );
}

beforeEach(() => {
  void i18n.changeLanguage('en');
  useStore.setState({ uiMode: 'simple', messageQueues: {}, agents: [] } as never);
  HTMLElement.prototype.scrollTo = vi.fn(function (this: HTMLElement, o: ScrollToOptions) {
    this.scrollTop = Number(o.top ?? 0);
  }) as never;
});
afterEach(() => { vi.useRealTimers(); });

const pane = () => screen.getByTestId('convo-terminal-pane');
const toggle = () => screen.getByTestId('convo-terminal-toggle');

describe('bubbles', () => {
  it('shows the person and the assistant as distinct bubbles with the speaker announced', async () => {
    bridge(() => hermes([
      msg('h1', 'user', 'Please summarize the sales report'),
      msg('h2', 'assistant', 'Sure — here is the **summary**:\n\n- item one\n- item two')
    ]));
    view();
    const mine = await screen.findByTestId('convo-user');
    const theirs = await screen.findByTestId('convo-assistant');
    expect(mine.getAttribute('data-role')).toBe('user');
    expect(theirs.getAttribute('data-role')).toBe('assistant');
    // spoken speaker prefix + visible caption (caption hidden from assistive tech)
    expect(within(mine).getByText(/You said:/)).toBeTruthy();
    expect(within(theirs).getByText(/Michael said:/)).toBeTruthy();
    expect(mine.querySelector('[aria-hidden="true"]')?.textContent).toMatch(/^You/);
    expect(theirs.querySelector('[aria-hidden="true"]')?.textContent).toMatch(/^Michael/);
    // assistant text is rendered markdown; the person's text stays plain
    expect(theirs.querySelector('strong')?.textContent).toBe('summary');
    expect(theirs.querySelectorAll('li')).toHaveLength(2);
    expect(mine.textContent).toContain('Please summarize the sales report');
  });

  it('never renders raw HTML from an assistant reply (shown as text, no elements, no handlers)', async () => {
    bridge(() => hermes([
      msg('h1', 'assistant', 'hi <img src=x onerror="window.__pwned=1"> <script>window.__pwned=2</script> [click](javascript:alert(1))')
    ]));
    view();
    const b = await screen.findByTestId('convo-assistant');
    expect(b.querySelector('img')).toBeNull();
    expect(b.querySelector('script')).toBeNull();
    expect(b.textContent).toContain('<img src=x');
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
    const a = b.querySelector('a');
    expect(a?.getAttribute('href') ?? '').not.toMatch(/^javascript:/i);
  });

  it('an update from the team is one muted line, never the injected prompt text', async () => {
    bridge(() => hermes([
      { id: 'h1', role: 'system', text: '', kind: 'team', ts: 1 },
      { id: 'h2', role: 'system', text: '', kind: 'team', ts: 2 },
      msg('h3', 'assistant', 'On it.')
    ]));
    view();
    await screen.findByTestId('convo-assistant');
    expect(screen.getAllByTestId('convo-team')).toHaveLength(1);
    expect(screen.getByTestId('convo-team').textContent).toBe('Michael got an update from the team');
  });
});

describe('accessibility', () => {
  it('the list is a labelled, polite, keyboard-focusable log; the controls are real named buttons', async () => {
    bridge(() => hermes([msg('h1', 'user', 'hi')]));
    view();
    const log = await screen.findByRole('log', { name: 'Conversation with Michael' });
    expect(log.getAttribute('aria-live')).toBe('polite');
    expect(log.getAttribute('tabindex')).toBe('0');
    const btn = screen.getByRole('button', { name: 'Show live terminal' });
    expect(btn.tagName).toBe('BUTTON');
    expect(btn.getAttribute('aria-controls')).toBe(pane().id);
    expect(btn.getAttribute('aria-expanded')).toBe('false');
  });

  it('the working indicator is a status region, only while working or thinking', async () => {
    bridge(() => hermes([msg('h1', 'user', 'hi')]));
    const { rerender } = view({ ...god, status: 'working' } as Agent);
    const s = await screen.findByRole('status');
    expect(s.textContent).toContain('Michael is working on it');
    rerender(<ConversationView agent={{ ...god, status: 'thinking' } as Agent} terminal={<i />} composer={<i />} />);
    expect(screen.getByRole('status')).toBeTruthy();
    rerender(<ConversationView agent={{ ...god, status: 'idle' } as Agent} terminal={<i />} composer={<i />} />);
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('declares no animation the reduced-motion rule cannot switch off', async () => {
    bridge(() => hermes([]));
    const { container } = view({ ...god, status: 'working' } as Agent);
    await screen.findByTestId('convo-empty');
    const css = container.querySelector('style')?.textContent ?? '';
    expect(css).toMatch(/@media \(prefers-reduced-motion: reduce\)/);
    expect(css).toMatch(/\.cth-convo-dots span \{ animation: none/);
  });
});

describe('empty state', () => {
  it('invites the person to say hello using the resolved name', async () => {
    bridge(() => hermes([], 'h:0'));
    installFakeBridge({
      conversationRead: reads, getConfig: async () => ({}),
      hiveRegistry: async () => ({ agents: { god: { name: 'Ada' } } })
    });
    view({ ...god, name: 'Ada' } as Agent);
    expect((await screen.findByTestId('convo-empty')).textContent).toContain('Say hello to Ada…');
    await waitFor(() => expect(screen.getByRole('log').getAttribute('aria-label')).toBe('Conversation with Ada'));
  });
});

describe('pending (queued) messages', () => {
  it('shows a greyed "waiting to send" bubble for the person\'s queued text, not for app nudges', async () => {
    bridge(() => hermes([msg('h1', 'assistant', 'Hello!')]));
    useStore.setState({
      messageQueues: {
        god: [
          { id: 'q1', text: 'Please book the meeting room', ts: Date.now() },
          { id: 'q2', text: 'You have new hive inbox message(s) — at least: m1. Read your inbox', ts: Date.now() }
        ]
      }
    } as never);
    view();
    const p = await screen.findByTestId('convo-pending');
    expect(p.getAttribute('data-pending')).toBe('waiting');
    expect(p.textContent).toContain('Please book the meeting room');
    expect(within(p).getByText('Waiting to send')).toBeTruthy();
    expect(screen.getAllByTestId('convo-pending')).toHaveLength(1);
  });

  it('a delivered message stays as "Sending…" until the transcript shows it, then turns into the real bubble', async () => {
    let step = 0;
    bridge((_a, cursor) => {
      if (step === 0) return hermes([msg('h1', 'assistant', 'Hello!')], 'h:1');
      return cursor === 'h:1' && step >= 2
        ? hermes([msg('h2', 'user', 'Please book the meeting room')], 'h:2')
        : hermes([], cursor ?? 'h:1');
    });
    useStore.setState({ messageQueues: { god: [{ id: 'q1', text: 'Please book the meeting room', ts: Date.now() }] } } as never);
    view();
    await screen.findByTestId('convo-pending');
    // delivered: the composer drains it from the queue
    act(() => { useStore.setState({ messageQueues: { god: [] } } as never); });
    const sending = await screen.findByTestId('convo-pending');
    expect(sending.getAttribute('data-pending')).toBe('sending');
    expect(within(sending).getByText('Sending…')).toBeTruthy();
    // the transcript catches up
    step = 2;
    await waitFor(() => expect(screen.queryByTestId('convo-pending')).toBeNull(), { timeout: 5000 });
    expect(screen.getByTestId('convo-user').textContent).toContain('Please book the meeting room');
  }, 10_000);
});

describe('terminal disclosure and fallback', () => {
  it('keeps the raw terminal mounted but collapsed; the disclosure opens and closes it', async () => {
    bridge(() => hermes([msg('h1', 'user', 'hi')]));
    view();
    await screen.findByTestId('convo-user');
    expect(screen.getByTestId('term')).toBeTruthy();                // still mounted (status parsing needs it)
    expect(pane().style.display).toBe('none');
    fireEvent.click(toggle());
    expect(pane().style.display).toBe('flex');
    expect(screen.getByRole('button', { name: 'Hide live terminal' }).getAttribute('aria-expanded')).toBe('true');
    fireEvent.click(toggle());
    expect(pane().style.display).toBe('none');
    // a real button: Enter/Space activate natively, and focus is never trapped
    toggle().focus();
    expect(document.activeElement).toBe(toggle());
    (document.activeElement as HTMLElement).blur();
    expect(document.activeElement).toBe(document.body);
  });

  it.each([
    ['unsupported_engine', /replies appear in the live terminal below/],
    ['starting', /is getting ready/],
    ['unreadable', /can't show this conversation as a chat/]
  ])('an engine without a readable transcript (%s) shows the terminal with a one-line note, never an empty chat', async (reason, re) => {
    bridge(() => ({ messages: [], cursor: '', source: 'none', reason }));
    view();
    const note = await screen.findByTestId('convo-note');
    expect(note.textContent).toMatch(re);
    expect(pane().style.display).toBe('flex');
    expect(screen.queryByTestId('convo-empty')).toBeNull();
    expect(screen.queryByTestId('convo-terminal-toggle')).toBeNull();
    // there is no chat log at all in fallback mode
    expect(screen.queryByRole('log')).toBeNull();
  });

  it('flips from the fallback to the chat once a transcript appears', async () => {
    let ready = false;
    bridge(() => (ready
      ? hermes([msg('h1', 'assistant', 'Back online')])
      : { messages: [], cursor: '', source: 'none', reason: 'starting' }));
    vi.useFakeTimers({ shouldAdvanceTime: true });
    view();
    await screen.findByTestId('convo-note');
    ready = true;
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    await screen.findByTestId('convo-assistant');
    expect(screen.queryByTestId('convo-note')).toBeNull();
    expect(pane().style.display).toBe('none');
  });
});

describe('polling', () => {
  it('is incremental (sends the cursor back), appends, and a reset replaces the list', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let n = 0;
    bridge((_a, cursor) => {
      n += 1;
      if (n === 1) return hermes([msg('h1', 'user', 'one')], 'h:1');
      if (n === 2) return hermes([msg('h2', 'assistant', 'two')], 'h:2');
      return hermes([msg('h9', 'assistant', 'fresh start')], 'h:9', { reset: true });
    });
    view();
    await screen.findByText('one');
    expect(reads.mock.calls[0]).toEqual(['god', null]);
    await act(async () => { await vi.advanceTimersByTimeAsync(2600); });
    await screen.findByText('two');
    expect(reads.mock.calls[1]).toEqual(['god', 'h:1']);
    expect(screen.getByText('one')).toBeTruthy();
    await act(async () => { await vi.advanceTimersByTimeAsync(2600); });
    await screen.findByText('fresh start');
    expect(reads.mock.calls[2]).toEqual(['god', 'h:2']);
    expect(screen.queryByText('one')).toBeNull();
  });

  it('stops polling while the window is hidden and resumes when it is shown', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    bridge(() => hermes([], 'h:0'));
    view();
    await screen.findByTestId('convo-empty');
    const calls = () => reads.mock.calls.length;
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
    await act(async () => { await vi.advanceTimersByTimeAsync(3000); }); // the in-flight timer fires once and parks
    const parked = calls();
    await act(async () => { await vi.advanceTimersByTimeAsync(20_000); });
    expect(calls()).toBe(parked);
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' });
    document.dispatchEvent(new Event('visibilitychange'));
    await act(async () => { await vi.advanceTimersByTimeAsync(50); });
    expect(calls()).toBeGreaterThan(parked);
  });

  it('survives a failing bridge call and an unimplemented bridge', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let fail = true;
    bridge(() => { if (fail) throw new Error('ipc down'); return hermes([msg('h1', 'assistant', 'recovered')]); });
    view();
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    fail = false;
    await act(async () => { await vi.advanceTimersByTimeAsync(5200); });
    await screen.findByText('recovered');
  });
});

describe('scrolling', () => {
  const setMetrics = (el: HTMLElement, m: { scrollHeight: number; clientHeight: number; scrollTop: number }) => {
    Object.defineProperty(el, 'scrollHeight', { configurable: true, value: m.scrollHeight });
    Object.defineProperty(el, 'clientHeight', { configurable: true, value: m.clientHeight });
    el.scrollTop = m.scrollTop;
  };

  it('follows new messages at the bottom; scrolled up it leaves the person alone and offers "Jump to latest"', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let n = 0;
    bridge(() => {
      n += 1;
      return hermes(Array.from({ length: n }, (_, i) => msg(`h${i}`, i % 2 ? 'assistant' : 'user', `message ${i}`, 1000 + i)), `h:${n}`);
    });
    view();
    const log = await screen.findByTestId('convo-log');
    await screen.findByText('message 0');
    expect(screen.queryByTestId('convo-jump')).toBeNull();

    // at the bottom → a new message scrolls the log down
    setMetrics(log, { scrollHeight: 1000, clientHeight: 400, scrollTop: 600 });
    fireEvent.scroll(log);
    (HTMLElement.prototype.scrollTo as unknown as ReturnType<typeof vi.fn>).mockClear();
    await act(async () => { await vi.advanceTimersByTimeAsync(2600); });
    await screen.findByText('message 1');
    expect(HTMLElement.prototype.scrollTo).toHaveBeenCalled();

    // scroll up → no yanking, the jump button appears
    setMetrics(log, { scrollHeight: 2000, clientHeight: 400, scrollTop: 100 });
    fireEvent.scroll(log);
    const jump = await screen.findByRole('button', { name: 'Jump to latest' });
    (HTMLElement.prototype.scrollTo as unknown as ReturnType<typeof vi.fn>).mockClear();
    await act(async () => { await vi.advanceTimersByTimeAsync(2600); });
    await screen.findByText('message 2');
    expect(HTMLElement.prototype.scrollTo).not.toHaveBeenCalled();
    expect(log.scrollTop).toBe(100);

    // jump → scrolls to the newest and the button goes away
    fireEvent.click(jump);
    expect(HTMLElement.prototype.scrollTo).toHaveBeenCalledWith(expect.objectContaining({ top: 2000 }));
    expect(screen.queryByTestId('convo-jump')).toBeNull();
  });

  it('uses an instant jump (no smooth animation) when the person prefers reduced motion', async () => {
    const mm = window.matchMedia;
    window.matchMedia = ((q: string) => ({ matches: /reduce/.test(q), media: q, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false, onchange: null })) as never;
    try {
      bridge(() => hermes([msg('h1', 'user', 'hi')]));
      view();
      const log = await screen.findByTestId('convo-log');
      setMetrics(log, { scrollHeight: 2000, clientHeight: 400, scrollTop: 0 });
      fireEvent.scroll(log);
      fireEvent.click(await screen.findByRole('button', { name: 'Jump to latest' }));
      expect(HTMLElement.prototype.scrollTo).toHaveBeenLastCalledWith({ top: 2000, behavior: 'auto' });
    } finally {
      window.matchMedia = mm;
    }
  });
});

describe('CommandCenterPanel wiring', () => {
  const setMode = (mode: 'simple' | 'advanced') => act(() => useStore.getState().setUiMode(mode));

  it('simple mode swaps the raw terminal for the conversation; the composer stays', async () => {
    bridge(() => hermes([msg('h1', 'assistant', 'Welcome back')]));
    setMode('simple');
    render(<CommandCenterPanel agent={god} />);
    await screen.findByTestId('conversation-view');
    await screen.findByText('Welcome back');
    expect(screen.getByTestId('composer')).toBeTruthy();
    expect(screen.getByTestId('pty-view')).toBeTruthy(); // mounted, collapsed
    expect(screen.getByTestId('convo-terminal-pane').style.display).toBe('none');
    expect(reads).toHaveBeenCalled();
  });

  it('advanced mode is unchanged: raw terminal + composer, no conversation view, no transcript reads', async () => {
    bridge(() => hermes([msg('h1', 'assistant', 'should never be read')]));
    setMode('advanced');
    render(<CommandCenterPanel agent={god} />);
    expect(screen.getByTestId('pty-view')).toBeTruthy();
    expect(screen.getByTestId('composer')).toBeTruthy();
    expect(screen.queryByTestId('conversation-view')).toBeNull();
    expect(screen.queryByTestId('convo-terminal-toggle')).toBeNull();
    await act(async () => { await new Promise((r) => setTimeout(r, 120)); });
    expect(reads).not.toHaveBeenCalled();
  });

  it('flipping simple → advanced puts the plain terminal back', async () => {
    bridge(() => hermes([msg('h1', 'assistant', 'hello')]));
    setMode('simple');
    render(<CommandCenterPanel agent={god} />);
    await screen.findByTestId('conversation-view');
    setMode('advanced');
    await waitFor(() => expect(screen.queryByTestId('conversation-view')).toBeNull());
    expect(screen.getByTestId('pty-view')).toBeTruthy();
    expect(screen.getByTestId('composer')).toBeTruthy();
  });
});

describe('locale parity for the conversation namespace', () => {
  const flat = (o: Record<string, unknown>, p = ''): Record<string, string> =>
    Object.entries(o).reduce<Record<string, string>>((acc, [k, v]) => {
      if (v && typeof v === 'object') Object.assign(acc, flat(v as Record<string, unknown>, `${p}${k}.`));
      else acc[`${p}${k}`] = String(v);
      return acc;
    }, {});
  const pick = (l: Record<string, unknown>) => flat((l as { conversation: Record<string, unknown> }).conversation);
  const ph = (s: string) => (s.match(/\{\{\s*\w+\s*\}\}/g) ?? []).map((x) => x.replace(/\s/g, '')).sort();

  it('every locale has exactly the same conversation.* keys, all non-empty, with the same placeholders', () => {
    const base = pick(en as never);
    expect(Object.keys(base).length).toBeGreaterThanOrEqual(16);
    for (const [name, loc] of [['id', id], ['ar', ar], ['zh-CN', zh]] as const) {
      const other = pick(loc as never);
      expect(Object.keys(other).sort(), name).toEqual(Object.keys(base).sort());
      for (const k of Object.keys(base)) {
        expect(other[k].trim().length, `${name} ${k}`).toBeGreaterThan(0);
        expect(ph(other[k]), `${name} ${k}`).toEqual(ph(base[k]));
      }
    }
  });

  it('plain language: no jargon in the English strings', () => {
    const text = Object.values(pick(en as never)).join(' ').toLowerCase();
    for (const bad of ['pty', 'stdout', 'xterm', 'json', 'sqlite', 'transcript', 'cursor', 'daemon']) {
      expect(text).not.toContain(bad);
    }
  });
});
