import { floorKey } from '@/floors';

/** Which store the roster is loaded FROM at boot — the file beside the hive, or
 *  this origin's localStorage.
 *
 *  Split out of store.ts because it is the one decision that has to be right on
 *  every launch and cannot be observed from the UI when it is wrong: the roster
 *  just appears, and nothing says where it came from.
 *
 *  localStorage is partitioned by ORIGIN, not by hive. Every hive this app ever
 *  opens shares exactly one of them, so it can only ever hold the roster of
 *  whichever hive wrote it last. Adopting it into a DIFFERENT hive is how one
 *  workspace's team turns up in another — restorable entries and all, each one
 *  still carrying the `cwd` of the workspace it was hired in. */

/** The localStorage keys that hold one hive's roster (agents, archive, restorable
 *  list, selection, parked queues). store.ts persists to these; a workspace switch
 *  clears them so the old hive's team is not adopted into the new one. */
export const ROSTER_STORAGE_KEYS = [
  'cth.agents',
  'cth.archivedAgents',
  'cth.restorableAgents',
  'cth.selectedId',
  'cth.messageQueues'
] as const;

/** The roster keys as THIS window uses them: `main` keeps the plain names, a
 *  multi-window floor gets its own copies (see src/floors.ts) so floors never
 *  share or overwrite each other's roster in the one WebView2 profile. */
export function rosterStorageKeys(label?: string): [string, string, string, string, string] {
  const [agents, archived, restorable, selected, queues] = ROSTER_STORAGE_KEYS;
  return [
    floorKey(agents, label),
    floorKey(archived, label),
    floorKey(restorable, label),
    floorKey(selected, label),
    floorKey(queues, label)
  ];
}

/** Remove every roster key and hand back an undo, for a switch that then fails. */
export function stashRosterStorage(
  storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
  label?: string
): () => void {
  const saved: Array<[string, string]> = [];
  for (const key of rosterStorageKeys(label)) {
    const v = storage.getItem(key);
    if (v !== null) saved.push([key, v]);
    storage.removeItem(key);
  }
  return () => { for (const [k, v] of saved) storage.setItem(k, v); };
}

export interface RosterCounts {
  agents: unknown[];
  archived: unknown[];
  restorable: unknown[];
}

export interface RosterSourceInput {
  /** `<harnessHome>/roster.json` as read at boot, or `null` when there is none. */
  fileRoster: RosterCounts | null;
  /** The hive this window is opening. `null` before onboarding picks one. */
  currentHome: string | null;
  /** The hive localStorage was last written for, or `null` when it predates the
   *  stamp (an install that upgraded into this behaviour). */
  storedHome: string | null;
}

export interface RosterSource {
  /** Load the slices out of the file. */
  useFileRoster: boolean;
  /** Load the slices out of localStorage — and, on a first run, seed the file
   *  from them. False means BOTH stores are ignored and the floor starts empty. */
  useLocalFallback: boolean;
}

/** An empty file must never win over a populated localStorage: that is the
 *  "opened the packaged build once and my floor went blank" failure the mirror
 *  exists to prevent (the two origins do not share a localStorage, the file is
 *  what bridges them). A genuine delete-all clears both, so nothing resurrects.
 *
 *  Guards are Array.isArray, not truthiness: a malformed roster.json ({}, a
 *  stray value, a file half-written by a crash) used to throw at module load
 *  and blank the WHOLE app before onboarding could even render. */
function fileHasRoster(file: RosterCounts | null): boolean {
  return !!file
    && (Array.isArray(file.agents) ? file.agents.length : 0)
      + (Array.isArray(file.archived) ? file.archived.length : 0)
      + (Array.isArray(file.restorable) ? file.restorable.length : 0)
      > 0;
}

export function chooseRosterSource({
  fileRoster,
  currentHome,
  storedHome
}: RosterSourceInput): RosterSource {
  if (fileHasRoster(fileRoster)) return { useFileRoster: true, useLocalFallback: false };

  // No file roster, so the only candidate left is localStorage — which is worth
  // reading only if it was written for THIS hive. `null` is the pre-stamp case:
  // an install whose localStorage predates the stamp is adopted once, because
  // refusing it would blank the floor of everyone upgrading.
  const belongsHere = storedHome === null || currentHome === null || storedHome === currentHome;
  return { useFileRoster: false, useLocalFallback: belongsHere };
}
