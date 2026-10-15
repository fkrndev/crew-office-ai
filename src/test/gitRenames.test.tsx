/**
 * Git status rows show a rename as "old → new": the Git tab renders it, and the
 * IDE changed-files list is built by changedFilesFrom/pathLabel (shared with the
 * tab's label) so the source path is not lost behind the new one.
 */
import { describe, it, expect } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { installFakeBridge } from './fakeBridge';
import { GitTab } from '@/components/GitTab';
import { changedFilesFrom, pathLabel } from '@shared/gitStatus';

const renamed = { index: 'R', worktree: ' ', oldPath: 'ren-old.txt', path: 'ren-new.txt' };

describe('git status renames', () => {
  it('pathLabel joins old and new with an arrow only for a real rename', () => {
    expect(pathLabel('ren-new.txt', 'ren-old.txt')).toBe('ren-old.txt → ren-new.txt');
    expect(pathLabel('a.txt')).toBe('a.txt');
    expect(pathLabel('a.txt', 'a.txt')).toBe('a.txt');
  });

  it('changedFilesFrom keeps oldPath, even when a later edit makes the unstaged state win', () => {
    const files = changedFilesFrom({
      staged: [renamed, { index: 'M', worktree: ' ', path: 'b.txt' }],
      unstaged: [{ index: 'R', worktree: 'M', path: 'ren-new.txt' }],
      untracked: ['z.txt']
    });
    expect(files).toEqual([
      { path: 'b.txt', code: 'M' },
      { path: 'ren-new.txt', code: 'M', oldPath: 'ren-old.txt' },
      { path: 'z.txt', code: '?' }
    ]);
    // a plain staged rename keeps its R code
    expect(changedFilesFrom({ staged: [renamed], unstaged: [], untracked: [] })).toEqual([
      { path: 'ren-new.txt', code: 'R', oldPath: 'ren-old.txt' }
    ]);
  });

  it('the Git tab renders a staged rename as old → new', async () => {
    installFakeBridge({
      gitIsRepo: async () => true,
      gitBranch: async () => ({ current: 'main', detached: false }),
      gitStatus: async () => ({
        staged: [renamed, { index: 'M', worktree: ' ', path: 'plain.txt' }],
        unstaged: [],
        untracked: []
      }),
      gitLog: async () => [],
      gitBranches: async () => ({ local: ['main'], remote: [] }),
      gitAheadBehind: async () => ({ ahead: 0, behind: 0, upstream: null })
    });
    render(<GitTab cwd="C:/repo" />);
    await waitFor(() => expect(screen.getByText('ren-old.txt → ren-new.txt')).toBeTruthy());
    expect(screen.getByText('plain.txt')).toBeTruthy();
  });
});
