/**
 * friendlyError: raw error strings -> {title, hint, action?, raw}. The inputs are
 * real-looking: the exact ureq / OS / Rust strings the backend produces
 * (src-tauri/src: "http status: 401", "io: Connection refused (os error 10061)",
 * "Permission denied (os error 13)", "cwd does not exist: …", the panic guard's
 * "command `x` panicked: …"), the `engine:setup` reason tokens and provider texts.
 */
import { describe, it, expect } from 'vitest';
import i18n from 'i18next';
import {
  FRIENDLY_KINDS, classifyError, friendlyError, rawErrorText, MAX_RAW, type FriendlyErrorKind
} from '@shared/friendlyError';
import en from '@/i18n/locales/en.json';
import id from '@/i18n/locales/id.json';
import ar from '@/i18n/locales/ar.json';
import zh from '@/i18n/locales/zh-CN.json';

const t = (k: string, o?: Record<string, unknown>): string => i18n.t(k, o) as string;

const TABLE: Array<[string, FriendlyErrorKind]> = [
  // ── provider key rejected (ureq StatusCode display + provider texts) ──
  ['http status: 401', 'key_rejected'],
  ['http status: 403', 'key_rejected'],
  ['github request failed: http status: 403', 'key_rejected'],
  ['rejected', 'key_rejected'],
  ['Error code: 401 - {"error": {"message": "Incorrect API key provided: sk-…abcd"}}', 'key_rejected'],
  ['openai.AuthenticationError: Error code: 401 - invalid api key', 'key_rejected'],
  ['{"error":{"code":"invalid_api_key","message":"nope"}}', 'key_rejected'],
  ['401 Unauthorized', 'key_rejected'],
  ['Forbidden', 'key_rejected'],
  ['anthropic: invalid x-api-key', 'key_rejected'],
  // ── no key at all ──
  ['no_key', 'no_key'],
  ['No API keys or providers found', 'no_key'],
  ['Hermes: missing API key', 'no_key'],
  ['no OpenAI API key set — add one in Settings → Voice', 'no_key'],
  // ── out of credit ──
  ['http status: 402', 'no_credit'],
  ['Insufficient credits. Add more at https://openrouter.ai/credits', 'no_credit'],
  ['You exceeded your current quota, please check your plan and billing details', 'no_credit'],
  // ── rate limit ──
  ['http status: 429', 'rate_limit'],
  ['rate limited', 'rate_limit'],
  ['Rate limit reached for requests', 'rate_limit'],
  ['429 Too Many Requests', 'rate_limit'],
  // ── the service itself ──
  ['http status: 500', 'server'],
  ['http status: 503', 'server'],
  ['502 Bad Gateway', 'server'],
  ['The server is overloaded', 'server'],
  // ── network / unreachable ──
  ['network', 'network'],
  ['io: Connection refused (os error 10061)', 'network'],
  ['io: A connection attempt failed because the connected party did not properly respond after a period of time (os error 10060)', 'network'],
  ['host not found', 'network'],
  ['connection failed', 'network'],
  ['update download failed: io: Connection reset by peer (os error 104)', 'network'],
  ['dns error: failed to lookup address information: Name or service not known', 'network'],
  ['getaddrinfo ENOTFOUND api.deepseek.com', 'network'],
  ['curl: (6) Could not resolve host: openrouter.ai', 'network'],
  ['Failed to fetch', 'network'],
  ['network is unreachable', 'network'],
  ['Hire fetch: error sending request for url (https://example.com/h.json)', 'network'],
  // ── timeout ──
  ['timeout', 'timeout'],
  ['timeout: global', 'timeout'],
  ['timeout: receive response', 'timeout'],
  ['Codex daemon command timed out after 30000ms', 'timeout'],
  ['hidden session timed out', 'timeout'],
  ['npm install failed or timed out', 'timeout'],
  // ── engine: setup reason tokens (engine_setup.rs) ──
  ['engine_missing', 'engine_missing'],
  ['engine_exit', 'engine_exit'],
  ['spawn_error', 'spawn_error'],
  ['installer_error', 'install_failed'],
  ['installer_unavailable', 'install_unavailable'],
  ['cli_missing', 'engine_missing'],
  ['cwd_missing', 'folder_missing'],
  ['already_running', 'already_running'],
  // ── engine: free text ──
  ["bash: hermes: command not found", 'engine_missing'],
  ["'hermes' is not recognized as an internal or external command, operable program or batch file.", 'engine_missing'],
  ['spawn: No such file or directory (os error 2)', 'engine_missing'],
  ['The process exited with code 1', 'engine_exit'],
  ['agent exited with exit code 127', 'engine_exit'],
  ['spawn failed — The directory name is invalid. (os error 267)', 'spawn_error'],
  ['openpty: Access is denied. (os error 5)', 'permission'],
  ['engine download failed: http status: 404', 'install_failed'],
  // ── the app's own spawn / store messages ──
  ['cwd does not exist: C:\\Users\\me\\Gone', 'folder_missing'],
  ['pty already exists for id pty-jim', 'already_running'],
  ['credential store did not persist the key', 'key_store'],
  ['key required', 'key_store'],
  // ── panic guard ──
  ['command `pty_spawn` panicked: called `Option::unwrap()` on a `None` value', 'internal'],
  ['command `control_halt` panicked: resolver consumed', 'internal'],
  ['thread \'tokio-runtime-worker\' panicked at src/commands.rs:123:5', 'internal'],
  // ── disk / permissions ──
  ['Permission denied (os error 13)', 'permission'],
  ['write C:\\Users\\me\\HarnessAgents\\config.json: Access is denied. (os error 5)', 'permission'],
  ['EACCES: permission denied, open \'/etc/hosts\'', 'permission'],
  ['Operation not permitted (os error 1)', 'permission'],
  ['No space left on device (os error 28)', 'disk_full'],
  ['write text.md: There is not enough space on the disk. (os error 112)', 'disk_full'],
  ['No such file or directory (os error 2)', 'not_found'],
  ['read failed: The system cannot find the path specified. (os error 3)', 'not_found'],
  // ── anything else ──
  ['something unexpected happened', 'generic'],
  ['', 'generic'],
  ['{"foo":1}', 'generic']
];

describe('classifyError', () => {
  it.each(TABLE)('%j -> %s', (input, kind) => {
    expect(classifyError(input)).toBe(kind);
  });

  it('accepts an Error, {error}, {message} and null', () => {
    expect(classifyError(new Error('http status: 401'))).toBe('key_rejected');
    expect(classifyError({ error: 'Permission denied (os error 13)' })).toBe('permission');
    expect(classifyError({ message: 'timeout: global' })).toBe('timeout');
    expect(classifyError(null)).toBe('generic');
    expect(classifyError(undefined)).toBe('generic');
  });

  it('is case-insensitive', () => {
    expect(classifyError('HTTP STATUS: 429')).toBe('rate_limit');
    expect(classifyError('PERMISSION DENIED')).toBe('permission');
  });

  it('does not mistake a plain number for a status', () => {
    expect(classifyError('wrote 4012 bytes')).toBe('generic');
    expect(classifyError('port 5000 is busy')).toBe('generic');
  });
});

describe('friendlyError', () => {
  it('returns a title, a hint and the raw text — and never the raw text in the headline', () => {
    const raw = 'github request failed: http status: 403';
    const fe = friendlyError(raw, t);
    expect(fe.kind).toBe('key_rejected');
    expect(fe.title).toBe("Your AI key wasn't accepted");
    expect(fe.hint).toMatch(/Copy it again/);
    expect(fe.raw).toBe(raw);
    expect(fe.title + fe.hint).not.toMatch(/http status|403|github/i);
  });

  it('never puts a stack trace, a panic text or JSON in the main text', () => {
    for (const [input] of TABLE) {
      const fe = friendlyError(input, t);
      const main = `${fe.title} ${fe.hint}`;
      expect(main).not.toMatch(/panicked|os error|http status|\{|\}|undefined|errors\./);
      expect(fe.title.length).toBeGreaterThan(3);
      expect(fe.hint.length).toBeGreaterThan(3);
    }
  });

  it('the panic guard text is "something went wrong inside the app", raw kept for Details', () => {
    const raw = 'command `pty_spawn` panicked: boom';
    const fe = friendlyError(raw, t);
    expect(fe.kind).toBe('internal');
    expect(fe.title).toBe('Something went wrong inside the app');
    expect(fe.raw).toBe(raw);
  });

  it('offers one action: a key problem -> check the key; a transient one -> try again', () => {
    expect(friendlyError('http status: 401', t).action).toEqual({ id: 'addKey', label: 'Check my key' });
    expect(friendlyError('no_key', t).action?.id).toBe('addKey');
    expect(friendlyError('installer_unavailable', t).action?.id).toBe('openSettings');
    expect(friendlyError('network', t).action).toEqual({ id: 'retry', label: 'Try again' });
    expect(friendlyError('Permission denied (os error 13)', t).action).toBeUndefined();
    // out of credit is fixed on the provider's website, not by re-pasting a key
    expect(friendlyError('http status: 402', t).action).toBeUndefined();
  });

  it('drops "Try again" when the host cannot retry', () => {
    expect(friendlyError('network', t, { canRetry: false }).action).toBeUndefined();
    expect(friendlyError('http status: 401', t, { canRetry: false }).action?.id).toBe('addKey');
  });

  it('uses the context for the headline of an error it cannot classify', () => {
    expect(friendlyError('weird', t, { context: 'save' }).title).toBe("Couldn't save your changes");
    expect(friendlyError('weird', t, { context: 'send' }).title).toBe("Couldn't send your message");
    expect(friendlyError('weird', t, { context: 'spawn' }).title).toBe("Couldn't add your teammate");
    expect(friendlyError('weird', t, { context: 'key' }).title).toBe("Couldn't save your key");
    expect(friendlyError('weird', t, { context: 'stop' }).title).toBe("Couldn't stop everything");
    expect(friendlyError('weird', t, { context: 'start' }).title).toBe("Couldn't start your assistant");
    expect(friendlyError('weird', t, { context: 'load' }).title).toBe("Couldn't open the app's settings");
    expect(friendlyError('weird', t).title).toBe('Something went wrong');
    // …but a known kind keeps its own headline whatever the context.
    expect(friendlyError('http status: 429', t, { context: 'save' }).title).toBe('The AI service is busy right now');
  });
});

describe('rawErrorText', () => {
  it('flattens objects, strips ANSI + control characters and caps the length', () => {
    expect(rawErrorText({ error: 'x' })).toBe('x');
    expect(rawErrorText(new Error('boom'))).toBe('boom');
    expect(rawErrorText('\u001b[31mred\u001b[0m text\u0007')).toBe('red text');
    const long = 'a'.repeat(MAX_RAW * 2);
    expect(rawErrorText(long).length).toBe(MAX_RAW + 1);
    expect(rawErrorText({ weird: true })).toBe('{"weird":true}');
  });
});

describe('errors.* locale parity', () => {
  const flat = (o: unknown, p = ''): Record<string, string> => {
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(o as Record<string, unknown>)) {
      if (v && typeof v === 'object') Object.assign(out, flat(v, `${p}${k}.`));
      else out[`${p}${k}`] = String(v);
    }
    return out;
  };
  const ph = (s: string): string[] => (s.match(/\{\{\s*[\w.]+\s*\}\}/g) ?? []).sort();
  const sets = { en, id, ar, zh };

  it('every kind has a title and a hint in all four languages', () => {
    for (const [name, loc] of Object.entries(sets)) {
      const f = flat((loc as Record<string, unknown>).errors);
      for (const k of FRIENDLY_KINDS) {
        expect(f[`${k}.title`], `${name} ${k}.title`).toBeTruthy();
        expect(f[`${k}.hint`], `${name} ${k}.hint`).toBeTruthy();
      }
      for (const a of ['addKey', 'retry', 'openSettings']) expect(f[`action.${a}`], `${name} action.${a}`).toBeTruthy();
    }
  });

  it('identical key sets and placeholders across en / id / ar / zh-CN, nothing empty', () => {
    const base = flat(en.errors);
    for (const [name, loc] of Object.entries(sets)) {
      const f = flat((loc as Record<string, unknown>).errors);
      expect(Object.keys(f).sort(), name).toEqual(Object.keys(base).sort());
      for (const [k, v] of Object.entries(f)) {
        expect(v.trim(), `${name} ${k}`).not.toBe('');
        expect(ph(v), `${name} ${k}`).toEqual(ph(base[k]));
      }
    }
  });
});
