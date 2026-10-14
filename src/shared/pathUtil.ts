/**
 * Path helpers that survive Windows. The renderer sees paths from three
 * sources — the OS dialog (`C:\Users\me\proj`), agent output (`src/a.ts`,
 * `C:/proj/a.ts`), and registry cwds — and the old `startsWith(cwd + '/')`
 * checks matched only one spelling of one of them.
 *
 * Pure and platform-parameterised so it is testable without a window.
 */

/** True on a Windows host. Reads the bridge's platform, falling back to the UA. */
export function isWindowsHost(): boolean {
  try {
    const p = (window as unknown as { cth?: { platform?: string } }).cth?.platform;
    if (p) return p === 'win32';
    return typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent);
  } catch {
    return false;
  }
}

/** Forward-slash spelling with no trailing separator (a bare drive/root keeps it). */
export function normalizeSeparators(p: string): string {
  const s = p.replace(/\\/g, '/');
  return s.length > 1 && !/^[A-Za-z]:\/$/.test(s) ? s.replace(/\/+$/, '') : s;
}

/** Absolute in any spelling we meet: `/x`, `~/x`, `C:\x`, `C:/x`, `\\server\share`. */
export function isAbsolutePath(p: string): boolean {
  return p.startsWith('/') || p.startsWith('~/') || p.startsWith('\\\\') || /^[A-Za-z]:[\\/]/.test(p);
}

/** `base` + `rel`, using the separator `base` already uses. */
export function joinPath(base: string, rel: string): string {
  const sep = base.includes('\\') && !base.includes('/') ? '\\' : '/';
  const tail = rel.replace(/^\.[\\/]/, '').replace(/[\\/]+/g, sep);
  return `${base.replace(/[\\/]+$/, '')}${sep}${tail}`;
}

/**
 * `path` relative to `dir` (forward slashes) when it lies inside it, else null.
 * Separators are normalised; comparison ignores case on Windows, where
 * `C:\Proj` and `c:\proj` are the same folder.
 */
export function relativeTo(
  dir: string,
  path: string,
  caseInsensitive: boolean = isWindowsHost()
): string | null {
  const d = normalizeSeparators(dir);
  const p = normalizeSeparators(path);
  const key = (s: string) => (caseInsensitive ? s.toLowerCase() : s);
  const prefix = d.endsWith('/') ? d : `${d}/`;
  if (key(p) === key(d)) return '';
  return key(p).startsWith(key(prefix)) ? p.slice(prefix.length) : null;
}

/** Last path segment (either separator) — "the folder name". */
export function lastSegment(p: string): string {
  const parts = (p || '').split(/[\\/]+/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : p;
}
