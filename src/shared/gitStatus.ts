/**
 * Display helpers for `git status` entries. A staged rename carries its source
 * in `oldPath` (git_ops.rs git_status); showing only the new path hides that the
 * file was moved, so rename rows read "old → new" (an arrow, no new strings).
 */

/** The path as shown in a status row: "old → new" for a rename, else the path. */
export function pathLabel(path: string, oldPath?: string): string {
  return oldPath && oldPath !== path ? `${oldPath} → ${path}` : path;
}

export interface StatusEntry { path: string; index: string; worktree: string; oldPath?: string }
export interface StatusLike { staged: StatusEntry[]; unstaged: StatusEntry[]; untracked: string[] }
export interface ChangedFile { path: string; code: string; oldPath?: string }

/**
 * One row per changed path (unstaged state wins, then staged, then untracked),
 * sorted by path. `oldPath` comes from the staged entry even when the unstaged
 * state wins the code — a renamed file edited afterwards is still a rename.
 */
export function changedFilesFrom(status: StatusLike): ChangedFile[] {
  const map = new Map<string, ChangedFile>();
  for (const e of status.unstaged) map.set(e.path, { path: e.path, code: e.worktree });
  for (const e of status.staged) {
    const cur = map.get(e.path);
    if (!cur) map.set(e.path, { path: e.path, code: e.index });
    if (e.oldPath) map.get(e.path)!.oldPath = e.oldPath;
  }
  for (const p of status.untracked) if (!map.has(p)) map.set(p, { path: p, code: '?' });
  return [...map.values()].sort((a, b) => a.path.localeCompare(b.path));
}
