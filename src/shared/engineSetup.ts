/**
 * First-run engine setup — the frontend half of `engine:setup`
 * (src-tauri/src/engine_setup.rs). The backend turns the missing-CLI install
 * ladder and the engine's first start into one structured event; this file is
 * everything about it that is pure: the event shape, the per-agent reducer, the
 * step counter, and what each failure reason asks the user to do. No React, no
 * store, no bridge (the bridge is injected where needed) so all of it is unit-
 * testable with a fake event source.
 */

export type SetupPhase =
  | 'checking'
  | 'installing'
  | 'starting'
  | 'preparing'
  | 'ready'
  | 'failed'
  | 'stopped';

/** Phases that keep a card on screen (ready / stopped clear the state). */
export type ActiveSetupPhase = 'checking' | 'installing' | 'starting' | 'preparing' | 'failed';

/** Closed enum from `classify_failure` + the ladder (engine_setup.rs). */
export type SetupFailureReason =
  | 'network'
  | 'no_key'
  | 'installer_error'
  | 'installer_unavailable'
  | 'engine_missing'
  | 'engine_exit'
  | 'timeout'
  | 'spawn_error';

export interface EngineSetupEvent {
  agentId: string;
  engine: string;
  phase: SetupPhase;
  /** Machine token for the step (`native`, `python`, `preparing-deps`, …), or
   *  the last output line on `failed`. */
  detail?: string;
  ts?: number;
  startedAt?: number;
  reason?: SetupFailureReason | string;
  /** Newest cleaned output line (throttled by the backend). */
  line?: string;
  /** Snapshot only: the recent lines the backend remembers. */
  lines?: string[];
}

export interface EngineSetupState {
  agentId: string;
  engine: string;
  phase: ActiveSetupPhase;
  detail: string;
  startedAt: number;
  /** When the current phase began (ms, wall clock). */
  phaseSince: number;
  /** An installer step was seen: the counter then has three steps, else two. */
  sawInstall: boolean;
  reason?: SetupFailureReason;
  /** On failure: the phase it failed in. */
  failedIn?: Exclude<ActiveSetupPhase, 'failed'>;
  /** Recent raw output, newest last (the "Show details" disclosure). */
  lines: string[];
  /** This attempt follows a failure (the user pressed Try again): the card stays
   *  on screen through it instead of vanishing for the quick-start grace period. */
  retry?: boolean;
}

export const MAX_LINES = 12;

const PHASES: ReadonlySet<string> = new Set([
  'checking', 'installing', 'starting', 'preparing', 'ready', 'failed', 'stopped'
]);
const REASONS: ReadonlySet<string> = new Set([
  'network', 'no_key', 'installer_error', 'installer_unavailable',
  'engine_missing', 'engine_exit', 'timeout', 'spawn_error'
]);

export function isSetupEvent(v: unknown): v is EngineSetupEvent {
  const e = v as EngineSetupEvent | null;
  return !!e && typeof e === 'object' && typeof e.agentId === 'string' && !!e.agentId &&
    typeof e.phase === 'string' && PHASES.has(e.phase);
}

function pushLines(prev: string[], ev: EngineSetupEvent): string[] {
  const out = [...prev];
  const add = (l: unknown) => {
    if (typeof l !== 'string') return;
    const s = l.trim();
    if (s && out[out.length - 1] !== s) out.push(s);
  };
  if (Array.isArray(ev.lines)) ev.lines.forEach(add);
  add(ev.line);
  return out.slice(-MAX_LINES);
}

/**
 * Fold one backend event into an agent's setup state. `undefined` = nothing to
 * show (no setup, or it just finished / was stopped). A `checking` event always
 * starts a fresh attempt (retry); anything arriving after a failure other than a
 * new attempt is ignored so a late line cannot un-fail the card.
 */
export function reduceSetupEvent(
  prev: EngineSetupState | undefined,
  ev: unknown
): EngineSetupState | undefined {
  if (!isSetupEvent(ev)) return prev;
  if (ev.phase === 'ready' || ev.phase === 'stopped') return undefined;
  const now = typeof ev.ts === 'number' ? ev.ts : Date.now();
  const fresh = ev.phase === 'checking' || !prev;
  const base = fresh ? undefined : prev;
  if (prev?.phase === 'failed' && ev.phase !== 'checking' && ev.phase !== 'failed') {
    // A snapshot replay after a failure, or a stray late line: keep the failure.
    return prev;
  }
  const phase = ev.phase as ActiveSetupPhase;
  const phaseChanged = !base || base.phase !== phase;
  const sawInstall = (base?.sawInstall ?? false) || ev.phase === 'installing';
  const next: EngineSetupState = {
    agentId: ev.agentId,
    engine: ev.engine || base?.engine || '',
    phase,
    detail: typeof ev.detail === 'string' ? ev.detail : (base?.detail ?? ''),
    startedAt: base?.startedAt ?? (typeof ev.startedAt === 'number' ? ev.startedAt : now),
    phaseSince: phaseChanged ? now : (base?.phaseSince ?? now),
    sawInstall,
    lines: pushLines(base?.lines ?? [], ev)
  };
  if (prev?.phase === 'failed' && ev.phase === 'checking') next.retry = true;
  else if (base?.retry) next.retry = true;
  if (phase === 'failed') {
    next.reason = REASONS.has(String(ev.reason)) ? (ev.reason as SetupFailureReason) : 'engine_exit';
    next.failedIn = base && base.phase !== 'failed' ? base.phase : base?.failedIn;
    // The failure's own detail is the last output line: keep it visible too.
    if (typeof ev.detail === 'string' && ev.detail) next.lines = pushLines(next.lines, { ...ev, line: ev.detail });
  }
  return next;
}

// ─── the step counter ───────────────────────────────────────────────────────

export type StepKey = 'checking' | 'installing' | 'starting';

export interface SetupStep {
  /** 1-based. */
  current: number;
  total: number;
  key: StepKey;
}

/**
 * "Step 2 of 3". Honest about skipped work: when no installer ran (the engine
 * was already on the machine) there are two steps, not three. A failure reports
 * the step it failed in.
 */
export function setupStep(s: EngineSetupState): SetupStep {
  const total = s.sawInstall ? 3 : 2;
  const at = s.phase === 'failed' ? (s.failedIn ?? 'starting') : s.phase;
  if (at === 'checking') return { current: 1, total, key: 'checking' };
  if (at === 'installing') return { current: 2, total, key: 'installing' };
  return { current: total, total, key: 'starting' };
}

/** i18n key (under `firstRun.`) for the sentence that says what is happening. */
export function setupStepTextKey(s: EngineSetupState): string {
  if (s.phase === 'installing') {
    // The installer's sub-steps refine the sentence.
    if (['python', 'node', 'ripgrep', 'ffmpeg'].includes(s.detail)) return `firstRun.detail.${s.detail}`;
    return 'firstRun.step.installing';
  }
  if (s.phase === 'preparing') return 'firstRun.step.preparing';
  if (s.phase === 'starting') return 'firstRun.step.starting';
  return 'firstRun.step.checking';
}

// ─── failures ───────────────────────────────────────────────────────────────

/** The ONE button a failure offers. */
export type FailureAction = 'retry' | 'addKey' | 'openSettings';

export function failureAction(reason: SetupFailureReason | undefined): FailureAction {
  if (reason === 'no_key') return 'addKey';
  if (reason === 'installer_unavailable') return 'openSettings';
  return 'retry';
}

/** i18n key stem (`firstRun.failure.<stem>.title|body`) for a reason. */
export function failureStem(reason: SetupFailureReason | undefined): SetupFailureReason {
  return reason && REASONS.has(reason) ? reason : 'engine_exit';
}

// ─── when to show the card ──────────────────────────────────────────────────

/** A start that finishes in a couple of seconds (the engine was already set up)
 *  never needs a card; installing / preparing / failed always do. */
export const QUICK_START_GRACE_MS = 2500;

export function shouldShowSetup(s: EngineSetupState | undefined, now: number): boolean {
  if (!s) return false;
  if (s.retry) return true;
  if (s.phase === 'installing' || s.phase === 'preparing' || s.phase === 'failed') return true;
  return now - s.startedAt >= QUICK_START_GRACE_MS;
}

/** After this long the card admits it is taking a while. */
export const LONG_WAIT_MS = 4 * 60 * 1000;

// ─── bridge-touching helpers (the bridge is injected) ───────────────────────

export interface EngineSetupBridge {
  onEngineSetup?(cb: (e: EngineSetupEvent) => void): () => void;
  engineSetupSnapshot?(): Promise<EngineSetupEvent[] | undefined>;
  engineSetupRetry?(agentId: string): Promise<boolean | undefined>;
}

/** Subscribe, then replay what happened before this window was listening (a
 *  reload mid-install). Returns the unsubscribe. Failures are swallowed: a
 *  missing progress card must never break the app. */
export function listenEngineSetup(
  bridge: EngineSetupBridge,
  onEvent: (e: EngineSetupEvent) => void
): () => void {
  let live = true;
  const off = bridge.onEngineSetup?.((e) => { if (live) onEvent(e); });
  void Promise.resolve(bridge.engineSetupSnapshot?.())
    .then((rows) => { if (live && Array.isArray(rows)) rows.forEach((r) => onEvent(r)); })
    .catch(() => { /* no snapshot — live events only */ });
  return () => { live = false; off?.(); };
}

export type RetryResult = { ok: true } | { ok: false; error: string; noAgent: boolean };

/** "Try again": the backend stops whatever is half-alive and re-runs the whole
 *  spawn ladder for this agent (installer included). */
export async function retryEngineSetup(bridge: EngineSetupBridge, agentId: string): Promise<RetryResult> {
  try {
    const r = await bridge.engineSetupRetry?.(agentId);
    if (r === false) return { ok: false, error: 'rejected', noAgent: false };
    return { ok: true };
  } catch (e) {
    const error = e instanceof Error ? e.message : typeof e === 'string' ? e : JSON.stringify(e);
    return { ok: false, error, noAgent: /no agent/i.test(error) };
  }
}
