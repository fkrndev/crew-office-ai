/**
 * Friendly errors — turn the raw strings and codes the app can surface (a Rust
 * `Err(String)`, an ureq / OS error, a provider's HTTP status, an `engine:setup`
 * failure reason, the panic guard's "command `x` panicked: …") into what a
 * non-technical person can act on:
 *
 *   { title, hint, action?, raw }
 *
 * `title` says what happened in plain words, `hint` says what to do next and
 * neither ever contains a stack trace, a status line or JSON. The original text is
 * kept in `raw` so the UI can offer it behind a small "Details" disclosure.
 *
 * Pure: classification is string matching, wording comes from i18n (`errors.*`)
 * through an injected `t` (no React, no store), so the whole table is unit-tested.
 * Advanced mode keeps showing the raw text where it always did — the callers
 * decide (see components/FriendlyError.tsx).
 */

export type FriendlyErrorKind =
  | 'key_rejected'
  | 'no_key'
  | 'no_credit'
  | 'rate_limit'
  | 'server'
  | 'network'
  | 'timeout'
  | 'engine_missing'
  | 'engine_exit'
  | 'spawn_error'
  | 'install_failed'
  | 'install_unavailable'
  | 'already_running'
  | 'folder_missing'
  | 'key_store'
  | 'internal'
  | 'disk_full'
  | 'permission'
  | 'not_found'
  | 'generic';

/** Every kind (the i18n table `errors.<kind>.title|hint` must cover each one). */
export const FRIENDLY_KINDS: readonly FriendlyErrorKind[] = [
  'key_rejected', 'no_key', 'no_credit', 'rate_limit', 'server', 'network', 'timeout',
  'engine_missing', 'engine_exit', 'spawn_error', 'install_failed', 'install_unavailable',
  'already_running', 'folder_missing', 'key_store', 'internal', 'disk_full', 'permission',
  'not_found', 'generic'
];

/** What the one optional button does (the host wires the handler). */
export type FriendlyAction = 'addKey' | 'retry' | 'openSettings';

/** Where the error came from — only used to pick a better headline for an error
 *  the table cannot classify (`errors.generic.title_<context>`). */
export type FriendlyContext = 'save' | 'send' | 'spawn' | 'key' | 'load' | 'stop' | 'start';

export interface FriendlyError {
  kind: FriendlyErrorKind;
  title: string;
  hint: string;
  action?: { id: FriendlyAction; label: string };
  /** The original text, trimmed and capped (for the Details disclosure). */
  raw: string;
}

export type Translate = (key: string, opts?: Record<string, unknown>) => string;

/** Longest raw text kept for the disclosure. */
export const MAX_RAW = 600;

/** The exact tokens `engine:setup` failures arrive as (engine_setup.rs). */
const SETUP_REASONS: Readonly<Record<string, FriendlyErrorKind>> = {
  network: 'network',
  no_key: 'no_key',
  installer_error: 'install_failed',
  installer_unavailable: 'install_unavailable',
  engine_missing: 'engine_missing',
  engine_exit: 'engine_exit',
  timeout: 'timeout',
  spawn_error: 'spawn_error',
  // The closed `agent_spawn_failed.reason` enum of commands.rs `spawn_fail_reason`.
  cwd_missing: 'folder_missing',
  already_running: 'already_running',
  cli_missing: 'engine_missing'
};

/** Pull a readable string out of whatever was thrown / returned. Never JSON in the
 *  result for a plain Error / string / `{error}` / `{message}`; an unknown object
 *  is stringified (it lands in the Details only, never in the headline). */
export function rawErrorText(err: unknown): string {
  let s = '';
  if (err == null) s = '';
  else if (typeof err === 'string') s = err;
  else if (err instanceof Error) s = err.message || err.name;
  else if (typeof err === 'object') {
    const o = err as Record<string, unknown>;
    if (typeof o.error === 'string') s = o.error;
    else if (typeof o.message === 'string') s = o.message;
    else if (typeof o.reason === 'string') s = o.reason;
    else {
      try { s = JSON.stringify(err); } catch { s = String(err); }
    }
  } else s = String(err);
  // eslint-disable-next-line no-control-regex
  s = s.replace(/\u001b\[[0-9;?]*[ -/]*[@-~]/g, '').replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g, '');
  s = s.trim();
  return s.length > MAX_RAW ? `${s.slice(0, MAX_RAW)}…` : s;
}

const has = (s: string, ...needles: string[]): boolean => needles.some((n) => s.includes(n));
const re = (s: string, rx: RegExp): boolean => rx.test(s);

/**
 * Which situation is this? Order matters: the most specific, most actionable
 * answers first, the broad network / generic ones last.
 */
export function classifyError(err: unknown): FriendlyErrorKind {
  const raw = rawErrorText(err);
  const s = raw.toLowerCase();
  if (!s) return 'generic';

  // An exact engine:setup / spawn-failure token.
  const token = SETUP_REASONS[s];
  if (token) return token;

  // The panic guard (src-tauri/src/panic_guard.rs): "command `x` panicked: …".
  if (re(s, /command `[^`]*` panicked/) || has(s, 'panicked at', 'thread \'', 'rust_backtrace')) return 'internal';

  // Disk and permissions (OS wording on Windows, macOS and Linux).
  if (has(s, 'no space left', 'disk full', 'not enough space', 'there is not enough space', 'os error 28', 'os error 112', 'enospc')) {
    return 'disk_full';
  }
  if (has(s, 'permission denied', 'access is denied', 'os error 13', 'os error 5)', 'eacces', 'eperm', 'operation not permitted', 'read-only file system', 'os error 30')) {
    return 'permission';
  }

  // Our own spawn / key-store messages (commands.rs, misc.rs).
  if (s.startsWith('cwd does not exist') || has(s, 'cwd_missing')) return 'folder_missing';
  if (has(s, 'already exists for id', 'already_running') || re(s, /agent .* already exists/)) return 'already_running';
  if (has(s, 'credential store did not persist', 'keyring', 'secret service', 'credential manager', 'key required')) return 'key_store';

  // The provider says the key is wrong. ("invalid api key" is also how
  // engine_setup words a bad key — to the person it is the same fix.)
  if (
    re(s, /\bhttp status: ?40[13]\b/) ||
    re(s, /\b(status|code|error)[ :=]+40[13]\b/) ||
    has(s, 'unauthorized', 'unauthorised', 'forbidden', 'invalid api key', 'invalid_api_key', 'incorrect api key',
      'api key is invalid', 'api key not valid', 'authentication failed', 'authenticationerror', 'invalid x-api-key',
      'invalid token', 'bad credentials', 'key was rejected', 'key rejected', 'revoked') ||
    s === 'rejected' || s === 'invalid'
  ) {
    return 'key_rejected';
  }

  // No key at all.
  if (
    has(s, 'api key not set', 'api key is not set', 'api key required', 'needs your key', 'needs an api key') ||
    re(s, /\b(no|missing)\b.{0,24}\bapi keys?\b/)
  ) {
    return 'no_key';
  }

  // The account is out of credit.
  if (re(s, /\bhttp status: ?402\b/) || has(s, 'insufficient credit', 'insufficient_quota', 'insufficient funds', 'insufficient balance',
    'payment required', 'out of credit', 'no credits', 'billing', 'quota exceeded', 'exceeded your current quota')) {
    return 'no_credit';
  }

  // Too many requests.
  if (re(s, /\bhttp status: ?429\b/) || re(s, /\b(status|code|error)[ :=]+429\b/) || has(s, 'rate limit', 'rate_limit', 'ratelimit', 'rate-limited', 'rate limited', 'too many requests')) {
    return 'rate_limit';
  }

  // The service itself is struggling.
  if (re(s, /\bhttp status: ?5\d\d\b/) || re(s, /\b(status|code)[ :=]+5\d\d\b/) || has(s, 'bad gateway', 'service unavailable', 'gateway timeout', 'internal server error', 'overloaded')) {
    return 'server';
  }

  // The engine / its installer.
  if (
    has(s, 'engine_missing', 'command not found', 'is not recognized as an internal or external command', 'no such file or directory: \'hermes\'',
      'program not found', 'executable not found', 'cannot find the file specified', 'not found on path', 'not found in path') ||
    re(s, /\bcli_missing\b/) ||
    (re(s, /\b(spawn|launch|start|run)\b/) && has(s, 'enoent', 'no such file or directory', 'os error 2)', 'the system cannot find the file'))
  ) {
    return 'engine_missing';
  }
  if (has(s, 'installer_unavailable', 'no installer')) return 'install_unavailable';
  if (has(s, 'installer_error', 'install failed', 'installation failed', 'engine download failed', 'npm install failed', 'bundle download failed')) {
    return has(s, 'timed out', 'timeout') ? 'timeout' : 'install_failed';
  }
  if (has(s, 'engine_exit', 'exited with code', 'exit code', 'exited with status', 'process exited', 'terminated unexpectedly', 'segmentation fault')) {
    return 'engine_exit';
  }
  if (has(s, 'spawn_error', 'spawn failed', 'failed to spawn', 'spawn:', 'openpty:', 'could not start')) return 'spawn_error';

  // Offline / unreachable (ureq: "io: Connection refused (os error 10061)",
  // "host not found", "connection failed"; getaddrinfo / curl / TLS texts).
  if (
    has(s, 'network', 'unreachable', 'offline', 'connection refused', 'connection reset', 'connection closed', 'connection aborted',
      'connection failed', 'host not found', 'could not resolve', "couldn't resolve", 'name or service not known', 'no such host',
      'failed to lookup address', 'getaddrinfo', 'dns', 'enotfound', 'econnrefused', 'econnreset', 'ehostunreach', 'enetunreach',
      'error sending request', 'failed to connect', 'unable to connect', 'tls handshake', 'certificate', 'ssl connect error',
      'curl: (6)', 'curl: (7)', 'curl: (35)', 'curl: (56)', 'os error 10060', 'os error 10061', 'os error 10051', 'os error 10065',
      'os error 111', 'os error 101', 'os error 110', 'failed to fetch', 'networkerror', 'load failed') ||
    re(s, /\bio: .*(refused|reset|broken pipe|timed out)/)
  ) {
    // A connect that timed out is "can't reach", not "slow".
    return 'network';
  }
  if (has(s, 'timed out', 'timeout', 'etimedout', 'deadline exceeded', 'took too long', 'curl: (28)')) return 'timeout';

  if (has(s, 'no such file or directory', 'os error 2)', 'os error 3)', 'cannot find the path', 'enoent', 'file not found', 'path not found', 'no such file')) {
    return 'not_found';
  }
  return 'generic';
}

/** The one button a kind offers (`undefined` = none). `retry` is only meaningful
 *  when the host can retry; the component drops it otherwise. */
export function actionFor(kind: FriendlyErrorKind): FriendlyAction | undefined {
  switch (kind) {
    case 'key_rejected':
    case 'no_key':
      return 'addKey';
    case 'install_unavailable':
      return 'openSettings';
    case 'rate_limit':
    case 'server':
    case 'network':
    case 'timeout':
    case 'engine_missing':
    case 'engine_exit':
    case 'spawn_error':
    case 'install_failed':
    case 'key_store':
    case 'internal':
    case 'generic':
      return 'retry';
    default:
      return undefined;
  }
}

/** Map any error to the title / hint / action the UI shows. */
export function friendlyError(
  err: unknown,
  t: Translate,
  opts: { context?: FriendlyContext; canRetry?: boolean } = {}
): FriendlyError {
  const raw = rawErrorText(err);
  const kind = classifyError(err);
  const ctx = opts.context ? { context: opts.context } : undefined;
  const title = kind === 'generic' ? t('errors.generic.title', ctx) : t(`errors.${kind}.title`);
  const hint = t(`errors.${kind}.hint`);
  const id = actionFor(kind);
  const action = id && (id !== 'retry' || opts.canRetry !== false)
    ? { id, label: t(`errors.action.${id}`) }
    : undefined;
  return { kind, title, hint, action, raw };
}
