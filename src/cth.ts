// GENERATED window.cth shim — upstream preload method→channel map.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { stashRosterStorage } from '@/store/rosterSource';
import { listenTarget } from '@/floors';
import { normalizeToolsProbe } from '@shared/toolCatalog';

/** Where "open the release page" lands when the caller has no specific URL —
 *  must stay inside the repo update_open_release allows. */
const RELEASES_URL = 'https://github.com/fkrndev/crew-office-ai/releases/latest';

const warned = new Set<string>();
const missing = (name: string) => (..._args: unknown[]) => {
  if (!warned.has(name)) { warned.add(name); console.warn('[cth] unimplemented:', name); }
  return Promise.resolve(null);
};
/** Like `on`, but scoped to THIS window's label: the backend addresses a PTY's
 *  stream to the window that owns it (floors.rs), and an untargeted listener
 *  would hear another floor's output too. Broadcasts still reach it. */
const onHere = <T>(channel: string, cb: (e: T) => void): (() => void) => {
  const p = listen<T>(channel, (ev) => cb((ev as { payload: T }).payload), { target: listenTarget() });
  return () => { void p.then((u) => u()); };
};
const pack = (vals: unknown[], names: string[]): Record<string, unknown> => {
  const o: Record<string, unknown> = {};
  vals.forEach((v, i) => { if (names[i]) o[names[i]] = v; });
  return o;
};
const on = <T>(channel: string, cb: (e: T) => void): (() => void) => {
  const p = listen<T>(channel, (ev) => cb((ev as { payload: T }).payload));
  return () => { void p.then((u) => u()); };
};

// Sync reads — upstream's sendSync surface (clipboard text, roster, harness
// home) is served from async-warmed caches. The loopback sync-XHR server is gone
// (it answered every origin, the clipboard included), so a sync read can be one
// poll stale; warmCth() primes the caches before the app modules evaluate, and
// the pollers only run while the window is actually in use.
let clipboardCache = '';
let rosterCache: unknown = null;
let homeCache: string | null = null;
const clipboardPoll = (): Promise<void> =>
  invoke<string | null>('app_clipboard_read')
    .then((t) => { if (typeof t === 'string') clipboardCache = t; })
    .catch(() => { /* keep the previous cache */ });
const rosterPoll = (): Promise<void> =>
  invoke<unknown>('roster_read')
    .then((r) => { if (r && typeof r === 'object') rosterCache = r; })
    .catch(() => { /* keep the previous cache */ });
const homePoll = (): Promise<void> =>
  invoke<string | null>('config_home_sync')
    .then((h) => { if (typeof h === 'string' && h) homeCache = h; })
    .catch(() => { /* keep the previous cache */ });

/** Window is on screen AND focused — the only time a fresh clipboard/roster
 *  read can matter, so background polling skips every other moment. */
const windowActive = (): boolean =>
  document.visibilityState === 'visible' &&
  (typeof document.hasFocus !== 'function' || document.hasFocus());

/** Prime the sync caches (bounded wait). Call BEFORE importing modules that read
 *  window.cth at evaluation time (store.ts roster/home, config.ts). */
export function warmCth(timeoutMs = 1500): Promise<void> {
  const primed = Promise.all([rosterPoll(), clipboardPoll(), homePoll()]).then(() => undefined);
  return Promise.race([primed, new Promise<void>((r) => window.setTimeout(r, timeoutMs))]);
}

const errMsg = (e: unknown): string =>
  e instanceof Error ? e.message : typeof e === 'string' ? e : (() => {
    try { return JSON.stringify(e); } catch { return String(e); }
  })();
const isPlainObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v) &&
  !(v instanceof ArrayBuffer) && !ArrayBuffer.isView(v);
/** Upstream call sites pass ONE options object; newer ones pass positionally.
 *  Accept both (the hiveMessages convention). */
const packOpts = (vals: unknown[], names: string[]): Record<string, unknown> =>
  vals.length === 1 && isPlainObject(vals[0]) ? vals[0] : pack(vals, names);
const toArray = (v: unknown): string[] =>
  Array.isArray(v) ? v.map(String) : typeof v === 'string' && v ? [v] : [];
/** Upstream's `{ok:true,…}` / `{ok:false,error}` envelope around a command that
 *  resolves a bare value and signals failure by rejecting. */
const envelope = async <T>(
  p: Promise<T>,
  map: (v: T) => Record<string, unknown> = () => ({})
): Promise<Record<string, unknown>> => {
  try { return { ok: true, ...map(await p) }; } catch (e) { return { ok: false, error: errMsg(e) }; }
};
/** git read commands reject with the failure text, but the git panes test
 *  `'error' in result` — hand them that shape instead of a rejection. */
const gitSafe = <T>(p: Promise<T>): Promise<T | { error: string }> =>
  p.catch((e) => ({ error: errMsg(e) }));
const toBase64 = (data: ArrayBuffer | ArrayBufferView): string => {
  const bytes = data instanceof ArrayBuffer
    ? new Uint8Array(data)
    : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) {
    bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(bin);
};
const fromBase64 = (b64: string): Uint8Array => {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
};
/** root + rel using the root's own separator; refuses `..` escapes. */
const joinPath = (root: string, rel: string): string => {
  const parts = rel.split(/[\\/]+/).filter(Boolean);
  if (parts.some((p) => p === '..')) throw new Error('path escapes the workspace');
  const sep = root.includes('\\') && !root.includes('/') ? '\\' : '/';
  return [root.replace(/[\\/]+$/, ''), ...parts].join(sep);
};

// Drag-drop file paths — with dragDropEnabled the OS drop never reaches the
// DOM in WebView2, so onDragDropEvent's paths are all we get. We re-emit a
// synthetic DOM `drop` carrying File objects whose Electron-style `.path` is
// filled in, so the copied renderer's drag handlers work unmodified.
let dropPaths: string[] = [];
let dropExpiry: number | undefined;
let backgroundStarted = false;

export function installCth(): void {
  // One-time background wiring (installCth runs again in tests). The caches are
  // seeded here, then refresh on focus/copy and a slow visible-only interval.
  if (!backgroundStarted) {
    backgroundStarted = true;
    void clipboardPoll();
    void rosterPoll();
    void homePoll();
    window.setInterval(() => { if (windowActive()) void clipboardPoll(); }, 2000);
    window.setInterval(() => { if (windowActive()) void rosterPoll(); }, 15000);
    const refresh = () => { void clipboardPoll(); void rosterPoll(); };
    window.addEventListener('focus', refresh);
    document.addEventListener('visibilitychange', () => { if (windowActive()) refresh(); });
    // A copy/cut inside the app changes the clipboard right now — don't wait a tick.
    const soon = () => { window.setTimeout(() => void clipboardPoll(), 50); };
    document.addEventListener('copy', soon);
    document.addEventListener('cut', soon);

    // Tauri's OS-level file drop carries the absolute paths WebView2 won't.
    // Re-emit it as a DOM `drop` on the element under the cursor — the copied
    // upstream drop handlers then read File.path exactly like Electron's.
    void getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type !== 'drop' || p.paths.length === 0) return;
        dropPaths = [...p.paths];
        window.clearTimeout(dropExpiry);
        let synthesized = false;
        try {
          const dt = new DataTransfer();
          for (const path of p.paths) {
            const f = new File([], path.split(/[\\/]/).pop() ?? 'file');
            (f as File & { path: string }).path = path;
            dt.items.add(f);
          }
          // The event position is PHYSICAL pixels; elementFromPoint/clientX are CSS.
          const dpr = window.devicePixelRatio || 1;
          const x = p.position.x / dpr;
          const y = p.position.y / dpr;
          const el = document.elementFromPoint(x, y) ?? document.body;
          el.dispatchEvent(
            new DragEvent('drop', {
              dataTransfer: dt,
              bubbles: true,
              cancelable: true,
              clientX: x,
              clientY: y,
            })
          );
          synthesized = true;
        } catch {
          /* synthetic drop unsupported — pathForFile still has the queue */
        }
        // The synthetic Files already carry .path; a leftover queue would be
        // handed to the NEXT unrelated pasted file. On the fallback path keep it
        // briefly so pathForFile can still be served.
        if (synthesized) dropPaths = [];
        else dropExpiry = window.setTimeout(() => { dropPaths = []; }, 5000);
      })
      .catch(() => { /* drag events unsupported — pathForFile degrades */ });
  }

  const ua = navigator.userAgent;
  const platform = ua.includes('Windows') ? 'win32' : ua.includes('Mac') ? 'darwin' : 'linux';
  const arch = navigator.userAgent.includes('ARM64') || navigator.userAgent.includes('ARM')
    ? 'arm64'
    : 'x64';

  window.cth = {
    // Properties (not methods) — upstream exposes process.platform/arch.
    platform,
    arch,
    // webUtils.getPathForFile — WebView2 File objects expose the real path
    // on the non-standard `.path` property already.
    pathForFile: (file: File): string =>
      (file as File & { path?: string }).path ?? dropPaths.shift() ?? '',
    // Sync reads are cached (see clipboardPoll/rosterPoll/homePoll). Exact upstream signatures.
    readClipboardSync: (): string => clipboardCache,
    harnessHomeSync: (): string | null => homeCache,
    trackMessageSent: (...args: unknown[]) => invoke('analytics_message_sent', pack(args, ["event","props"])),
    spawnPty: (...args: unknown[]) => invoke('spawn_agent', pack(args, ["args"])),
    writePty: (...args: unknown[]) => invoke('pty_write', pack(args, ["id","data"])),
    resizePty: (...args: unknown[]) => invoke('pty_resize', pack(args, ["id","cols","rows"])),
    redrawPty: (...args: unknown[]) => invoke('pty_redraw', pack(args, ["id"])),
    killPty: (...args: unknown[]) => invoke('kill_agent', pack(args, ["id"])),
    listPtys: (...args: unknown[]) => invoke('pty_list', pack(args, [])),
    resolveSessionCwd: (...args: unknown[]) => invoke('session_resolve_cwd', pack(args, ["agentId"])),
    chooseFolder: (...args: unknown[]) => invoke('dialog_choose_folder', pack(args, [])),
    // Rust returns a bare bool (rejects on failure); callers read `.ok`.
    openTerminalAt: (...args: unknown[]) => envelope(invoke('terminal_open_at_folder', pack(args, ["path"]))),
    copyToClipboard: (...args: unknown[]) => invoke('app_clipboard_write', pack(args, ["text"])),
    readClipboard: (...args: unknown[]) => invoke('app_clipboard_read', pack(args, [])),
    getConfig: (...args: unknown[]) => invoke('get_config', pack(args, [])),
    updateConfig: (...args: unknown[]) => invoke('config_update', pack(args, ["patch"])),
    setAgentTokenCap: (...args: unknown[]) => invoke('config_set_agent_token_cap', pack(args, ["agentId","cap"])),
    setAgentCostCap: (...args: unknown[]) => invoke('config_set_agent_cost_cap', pack(args, ["agentId","cap"])),
    ensureHarnessHome: (...args: unknown[]) => invoke('config_ensure_home', pack(args, ["path"])),
    // A 'fresh' switch opens an empty workspace; the old hive's roster still sits
    // in this origin's localStorage and would be restored into the new one
    // (useRestoreTeam), so clear it first. Success relaunches the process and
    // never returns; a rejection puts the roster back.
    changeHome: async (...args: unknown[]) => {
      const o = pack(args, ["newHome","mode"]);
      let undo: (() => void) | null = null;
      if (o.mode === 'fresh') {
        try { undo = stashRosterStorage(window.localStorage); } catch { /* storage unavailable */ }
      }
      try {
        return await invoke('config_change_home', o);
      } catch (e) {
        undo?.();
        throw e;
      }
    },
    // fs_* return bare shapes and reject on failure; the IDE/file tree speak
    // upstream's {ok,...} envelope, so adapt here once.
    listDir: (root: string, rel: string) =>
      envelope(
        invoke<Array<Record<string, unknown>>>('fs_list', { root, rel }),
        (rows) => ({ entries: rows.map((e) => ({ ...e, isDir: e.kind === 'dir' })) })
      ),
    readFile: (root: string, rel: string) =>
      envelope(
        invoke<{ kind: string; data: string }>('fs_read', { root, rel }).then((r) => {
          if (r.kind !== 'text') throw new Error('binary file (not displayable)');
          return r;
        }),
        (r) => ({ content: r.data, path: joinPath(root, rel) })
      ),
    // (path) absolute, or (root, rel) confined under root - useWorkspaceImage.
    readBinary: (root: string, rel?: string) =>
      envelope(
        // joinPath throws on a `..` escape — inside the promise so it envelopes.
        Promise.resolve().then(() => invoke<{ base64: string; mime: string; size: number; path: string }>(
          'fs_read_binary',
          { path: rel === undefined ? root : joinPath(root, rel) }
        )),
        (r) => ({ bytes: fromBase64(r.base64), mime: r.mime, size: r.size, path: r.path })
      ),
    writeFile: (root: string, rel: string, content: string) =>
      envelope(invoke('fs_write_file', { root, rel, content }), (r) => (isPlainObject(r) ? r : {})),
    statAbs: (...args: unknown[]) => invoke('fs_stat_abs', pack(args, ["path"])),
    revealPath: (...args: unknown[]) => invoke('fs_reveal_path', pack(args, ["path"])),
    gitIsRepo: (...args: unknown[]) => invoke('git_is_repo', pack(args, ["cwd"])),
    // Not-a-repo rejects; upstream resolved null (callers `?? root` / `|| null`).
    gitMainRepo: (...args: unknown[]) => invoke('git_main_repo', pack(args, ["cwd"])).catch(() => null),
    gitBranch: (...args: unknown[]) => gitSafe(invoke('git_branch', pack(args, ["cwd"]))),
    gitStatus: (...args: unknown[]) => gitSafe(invoke('git_status', pack(args, ["cwd"]))),
    gitLog: (...args: unknown[]) => gitSafe(invoke('git_log', pack(args, ["cwd","n"]))),
    gitBranches: (...args: unknown[]) => gitSafe(invoke('git_branches', pack(args, ["cwd"]))),
    gitAheadBehind: (...args: unknown[]) => gitSafe(invoke('git_ahead_behind', pack(args, ["cwd"]))),
    // Rust's git_diff returns a unified-diff string; the diff editor wants the
    // two sides, so rebuild them: HEAD blob vs the working-tree file.
    gitDiff: async (cwd: string, rel: string) => {
      try {
        const head = await invoke<{ isBinary?: boolean; content?: string }>(
          'git_show_file', { cwd, rev: 'HEAD', path: rel }
        ).catch(() => ({ isBinary: false, content: '' }));
        let working = '';
        let isBinary = head.isBinary === true;
        try {
          const w = await invoke<{ kind: string; data: string }>('fs_read', { root: cwd, rel });
          if (w.kind === 'text') working = w.data; else isBinary = true;
        } catch (e) {
          // A deleted file has no working side; anything else is a real failure.
          if (/too large/i.test(errMsg(e))) throw e;
        }
        return { ok: true, isBinary, head: head.content ?? '', working };
      } catch (e) {
        return { ok: false, error: errMsg(e) };
      }
    },
    gitLogGraph: (...args: unknown[]) => gitSafe(invoke('git_log_graph', pack(args, ["cwd","n","skip"]))),
    gitCommitFiles: (...args: unknown[]) => gitSafe(invoke('git_commit_files', pack(args, ["cwd","sha"]))),
    gitShowFile: (...args: unknown[]) => envelope(
      invoke<Record<string, unknown>>('git_show_file', pack(args, ["cwd","rev","path"])),
      (r) => (isPlainObject(r) ? r : {})
    ),
    gitCompareRefs: (...args: unknown[]) => gitSafe(invoke('git_compare_refs', pack(args, ["cwd","base","head","mode"]))),
    gitWorktrees: (...args: unknown[]) => gitSafe(invoke('git_worktrees', pack(args, ["cwd"]))),
    // Tauri keys a raw `r#ref` parameter as `ref`.
    gitCheckout: (...args: unknown[]) => envelope(
      invoke<Record<string, unknown>>('git_checkout', pack(args, ["cwd","ref","detach"])),
      (r) => (isPlainObject(r) ? r : {})
    ),
    hiveRegistry: (...args: unknown[]) => invoke('hive_registry', pack(args, [])),
    hivePatchAgentRole: (...args: unknown[]) => invoke('hive_patch_agent_role', pack(args, ["id","role"])),
    hiveRenameAgent: (...args: unknown[]) => invoke('hive_rename_agent', pack(args, ["id","name"])),
    hiveSetAgentHold: (...args: unknown[]) => invoke('hive_set_agent_hold', pack(args, ["id","hold"])),
    hiveBoard: (...args: unknown[]) => invoke('hive_board', pack(args, [])),
    hiveTasks: (...args: unknown[]) => invoke('hive_tasks', pack(args, [])),
    hiveLog: (...args: unknown[]) => invoke('hive_log', pack(args, ["limit"])),
    hiveMemory: (...args: unknown[]) => invoke('read_memory', pack(args, ["agentId"])),
    hiveInbox: (...args: unknown[]) => invoke('hive_inbox', pack(args, ["agentId"])),
    hiveMessages: (...args: unknown[]) => {
      // Upstream callers pass an opts object ({agentId,id,limit,...}); newer
      // call sites pass positionally. Accept both.
      const first = args[0];
      const opts = typeof first === 'object' && first !== null
        ? first as Record<string, unknown>
        : pack(args, ["agentId","id","limit","includeArchived"]);
      return invoke('hive_messages', opts);
    },
    hiveAgentDirectory: (...args: unknown[]) => invoke('hive_agent_directory', pack(args, ["id"])),
    listWorkers: (...args: unknown[]) => invoke('workers_list', pack(args, [])),
    stopWorker: (...args: unknown[]) => invoke('workers_stop', pack(args, ["id"])),
    memoryStatus: (...args: unknown[]) => invoke('memory_status', pack(args, [])),
    // Native rows are bare {id,found,path}; join them with the catalog (see normalizeToolsProbe).
    toolsStatus: async (...args: unknown[]) => normalizeToolsProbe(await invoke('tools_status', pack(args, [])), platform),
    heroPayload: (...args: unknown[]) => invoke('hero_payload', pack(args, ["force"])),
    modelCatalog: (...args: unknown[]) => invoke('models_catalog', pack(args, ['force'])),
    // `cwds` is Vec<String>; callers hand a single cwd (or nothing).
    skillsLocal: (cwds?: unknown) => invoke('skills_local', { cwds: toArray(cwds) }),
    skillsCatalog: (...args: unknown[]) => invoke('skills_catalog', pack(args, ["force"])),
    skillsInstall: (...args: unknown[]) => invoke('skills_install', pack(args, ["url","name"])),
    skillsUninstall: (path: string, cwds?: unknown) =>
      invoke('skills_uninstall', { path, cwds: toArray(cwds) }),
    skillsBundled: (...args: unknown[]) => invoke('skills_bundled', pack(args, [])),
    skillsInstallBundled: (...args: unknown[]) => invoke('skills_install_bundled', pack(args, ["dirName"])),
    shardxStatus: () => invoke('shardx_status'),
    shardxSetToken: (...args: unknown[]) => invoke('shardx_set_token', pack(args, ["token"])),
    shardxSetup: () => invoke('shardx_setup'),
    shardxBootstrap: () => invoke('shardx_bootstrap'),
    shardxCleanup: () => invoke('shardx_cleanup'),
  shardxProfiles: () => invoke('shardx_profiles'),
    shardxDownloadLauncher: () => invoke('shardx_download_launcher'),
    shardxTokenPreview: () => invoke('shardx_token_preview'),
    shardxProxies: () => invoke('shardx_proxies'),
    shardxSetProfileProxy: (...args: unknown[]) => invoke('shardx_set_profile_proxy', pack(args, ["profileId","proxyId"])),
    shardxAutomations: () => invoke('shardx_automations'),
    shardxRunAutomation: (...args: unknown[]) => invoke('shardx_run_automation', pack(args, ["projectId"])),
    shardxStopAutomation: (...args: unknown[]) => invoke('shardx_stop_automation', pack(args, ["projectId"])),
    shardxAutomationRuns: () => invoke('shardx_automation_runs'),
    skillsReveal: (...args: unknown[]) => invoke('skills_reveal', pack(args, ["id"])),
    searchMemory: (...args: unknown[]) => invoke('search_memory', pack(args, ["query","wing","limit"])),
    memoryWakeUp: (...args: unknown[]) => invoke('memory_wake_up', pack(args, ["wing"])),
    mineNow: (...args: unknown[]) => invoke('mine_now', pack(args, ["agentId"])),
    reflectNow: (...args: unknown[]) => invoke('reflect_now', pack(args, ["agentId"])),
    // Clean user/assistant turns of an agent's engine transcript (simple-mode chat view).
    conversationRead: (...args: unknown[]) => invoke('conversation_read', pack(args, ["agentId","sinceCursor"])),
    kgStatus: (...args: unknown[]) => invoke('kg_status', pack(args, [])),
    kgList: (...args: unknown[]) => invoke('kg_list', pack(args, [])),
    kgSearch: (...args: unknown[]) => invoke('kg_search', pack(args, ["query","limit"])),
    kgGet: (...args: unknown[]) => invoke('kg_get', pack(args, ["id"])),
    kgRemove: (...args: unknown[]) => invoke('kg_remove', pack(args, ["id"])),
    kgAddFiles: (...args: unknown[]) => invoke('kg_add_files', pack(args, ["payload"])),
    kgIngestFiles: (...args: unknown[]) => invoke('kg_ingest_files', pack(args, ["payload"])),
    attachFiles: (...args: unknown[]) => invoke('dialog_attach_files', pack(args, [])),
    // Rust puts the decoded image on the OS clipboard and returns a bool. Accepts
    // a base64 string/data URL or {base64}; a `file` the backend may one day
    // return rides through so the composer can attach it.
    saveClipboardImage: (arg?: unknown) => {
      const base64 = isPlainObject(arg) ? arg.base64 : arg;
      return envelope(
        invoke<unknown>('clipboard_save_image', { base64: typeof base64 === 'string' ? base64 : '' }),
        (r) => (isPlainObject(r) ? r : {})
      );
    },
    // Pasted screenshot → PNG temp file under the harness home. Accepts a base64
    // string/data URL or {base64}; resolves {ok:true,path} | {ok:false,error}.
    clipboardImageToFile: (arg?: unknown) => {
      const base64 = isPlainObject(arg) ? arg.base64 : arg;
      return envelope(
        invoke<unknown>('clipboard_image_to_file', { base64: typeof base64 === 'string' ? base64 : '' }),
        (r) => (isPlainObject(r) ? r : {})
      ) as Promise<{ ok: boolean; path?: string; error?: string }>;
    },
    historyAdd: (...args: unknown[]) => invoke('command_history_add', packOpts(args, ["agentId","cwd","text"])),
    historyList: (...args: unknown[]) => invoke('command_history_list', packOpts(args, ["agentId","limit"])),
    historySearch: (...args: unknown[]) => invoke('command_history_search', packOpts(args, ["query","limit"])),
    hiveSend: (...args: unknown[]) => invoke('hive_send', pack(args, ["partial","from"])),
    // Deep-link source parked by the backend (Option<String>): the https
    // manifest URL, or null when nothing is queued.
    drainPendingHires: (...args: unknown[]) => invoke('hire_drain_pending', pack(args, [])),
    // Resolve a source URL into a validated manifest ({ok,manifest,...}); rejects
    // with the reason (SSRF block, bad JSON, invalid manifest).
    hireFetch: (url: string) => invoke('hire_fetch', { url }),
    // Multi-select picker -> {ok,manifests,errors,error?}. The backend validates
    // every selected file independently (valid neighbours survive an invalid
    // one) and names a bad file by its basename.
    importHireFiles: async () => {
      try {
        const r = await invoke<{
          ok?: boolean; manifests?: unknown[]; errors?: string[]; error?: string;
        }>('hire_open_files');
        return {
          ok: r?.ok === true,
          manifests: r?.manifests ?? [],
          errors: r?.errors ?? [],
          ...(r?.error ? { error: r.error } : {})
        };
      } catch (e) {
        return { ok: false, manifests: [], errors: [] as string[], error: errMsg(e) };
      }
    },
    // Kills every PTY and exits the app immediately (no closing-time protocol).
    forceQuit: () => invoke<boolean>('app_force_quit'),
    // Re-arms the closing-time protocol (same rail as startClosingTime).
    confirmClose: (...args: unknown[]) => invoke('app_confirm_close', pack(args, [])),
    cancelClose: (...args: unknown[]) => invoke('app_cancel_close', pack(args, [])),
    // Multi-window floors (floors.rs): `{ok}` — false when the multiWindow
    // flag is off. closeFloor is the CONFIRMED close of the calling floor.
    newFloor: (...args: unknown[]) => invoke('window_new_floor', pack(args, [])),
    closeFloor: (...args: unknown[]) => invoke('window_close_floor', pack(args, [])),
    startClosingTime: (...args: unknown[]) => invoke('closing_time_start', pack(args, [])),
    cancelClosingTime: (...args: unknown[]) => invoke('closing_time_cancel', pack(args, [])),
    resetAll: (...args: unknown[]) => invoke('app_reset_all', pack(args, [])),
    agentUsage: (...args: unknown[]) => invoke('hive_agent_usage', pack(args, ["agentId"])),
    agentContext: (...args: unknown[]) => invoke('hive_agent_context', pack(args, ["agentId"])),
    telemetryUsage: (...args: unknown[]) => invoke('telemetry_usage', pack(args, ["agentId"])),
    telemetrySpans: (...args: unknown[]) => invoke('telemetry_spans', pack(args, ["agentId"])),
    telemetrySnapshot: (...args: unknown[]) => invoke('telemetry_snapshot', pack(args, [])),
    setBreakerState: (...args: unknown[]) => invoke('control_set_breaker_state', pack(args, ["agentId","enabled"])),
    controlPause: (...args: unknown[]) => invoke('control_pause', pack(args, ["agentId","on"])),
    controlAutoDelivery: (...args: unknown[]) => invoke('control_auto_delivery', pack(args, ["agentId","paused"])),
    controlResume: (...args: unknown[]) => invoke('control_resume', pack(args, ["agentId"])),
    controlGateTool: (...args: unknown[]) => invoke('control_gate_tool', pack(args, ["agentId","tool","on"])),
    controlSteer: (...args: unknown[]) => invoke('control_steer', pack(args, ["agentId","text"])),
    controlHalt: (...args: unknown[]) => invoke('control_halt', pack(args, ["agentId"])),
    // Simple-mode "Stop everything": halt every live agent (orchestrator + workers),
    // hold queued messages; nothing is killed. `controlResumeStopped` lifts just that.
    controlStopAll: (...args: unknown[]) => invoke('control_stop_all', pack(args, [])),
    controlResumeStopped: (...args: unknown[]) => invoke('control_resume_stopped', pack(args, [])),
    controlSnapshot: (...args: unknown[]) => invoke('control_snapshot', pack(args, ["agentId"])),
    hiveAddTask: (...args: unknown[]) => invoke('hive_add_task', pack(args, ["fields"])),
    // Rust resolves the patched card / the remaining cards and rejects on
    // failure; callers test `.ok`.
    hivePatchTask: (...args: unknown[]) =>
      envelope(invoke('hive_patch_task', pack(args, ["id","patch"])), (task) => ({ task })),
    hiveDeleteTask: (...args: unknown[]) =>
      envelope(invoke('hive_delete_task', pack(args, ["id"])), (tasks) => ({ tasks })),
    listMissions: (...args: unknown[]) => invoke('missions_list', pack(args, [])),
    saveMissions: (...args: unknown[]) => invoke('missions_save', pack(args, ["missions"])),
    // Rust resolves the bare hit list; callers read {ok,results}.
    textSearch: (...args: unknown[]) =>
      envelope(invoke('hive_text_search', pack(args, ["query","limit"])), (results) => ({ results })),
    githubIssues: (...args: unknown[]) => invoke('github_issues', pack(args, ["repo"])),
    githubCIRuns: (...args: unknown[]) => invoke('github_ci_runs', pack(args, ["repo"])),
    setNotifications: (...args: unknown[]) => invoke('app_set_notifications', pack(args, ["enabled"])),
    openExternal: (...args: unknown[]) => invoke('app_open_external', pack(args, ["url"])),
    setLoginItem: (...args: unknown[]) => invoke('app_set_login_item', pack(args, ["enabled"])),
    hiveSetArchived: (...args: unknown[]) => invoke('hive_set_archived', pack(args, ["id","archived"])),
    // slack_start needs the listener port and returns a bare bool; Settings wants
    // {ok,url} (the tunnel URL lives in slack_status).
    slackStart: async (...args: unknown[]) => {
      const o = packOpts(args, ["port","channelId"]);
      try {
        await invoke('slack_start', { port: Number(o.port) || 3847, channelId: o.channelId || null });
        const st = await invoke<{ url?: string | null }>('slack_status').catch(() => null);
        return { ok: true, url: st?.url ?? undefined };
      } catch (e) {
        return { ok: false, error: errMsg(e) };
      }
    },
    slackStop: (...args: unknown[]) => invoke('slack_stop', pack(args, [])),
    slackStatus: (...args: unknown[]) => invoke('slack_status', pack(args, [])),
    // Accepts {channel,thread_ts|threadTs,text} or positional (channel,threadTs,text).
    slackReply: (...args: unknown[]) => {
      const o = packOpts(args, ["channel","threadTs","text"]);
      return invoke('slack_reply', { channel: o.channel, threadTs: o.threadTs ?? o.thread_ts, text: o.text });
    },
    slackReplyScriptPath: (...args: unknown[]) => invoke('slack_reply_script_path', pack(args, [])),
    slackSetConfig: (...args: unknown[]) => invoke('slack_set_config', pack(args, ["cfg"])),
    webhookStart: (...args: unknown[]) => invoke('webhook_start', pack(args, ["port"])),
    webhookStop: (...args: unknown[]) => invoke('webhook_stop', pack(args, [])),
    webhookStatus: (...args: unknown[]) => invoke('webhook_status', pack(args, [])),
    webhookGenerateSecret: (...args: unknown[]) => invoke('webhook_generate_secret', pack(args, [])),
    webhookSetConfig: (...args: unknown[]) => invoke('webhook_set_config', pack(args, ["secret","port","enabled"])),
    getContextTrigger: (...args: unknown[]) => invoke('triggers_get_context', pack(args, [])),
    setContextTrigger: (...args: unknown[]) => invoke('triggers_set_context', pack(args, ["ctx"])),
    listWebhooks: (...args: unknown[]) => invoke('webhooks_list', pack(args, [])),
    saveWebhooks: (...args: unknown[]) => invoke('webhooks_save', pack(args, ["webhooks"])),
    deleteWebhook: (...args: unknown[]) => invoke('webhooks_delete', pack(args, ["id"])),
    generateWebhookSecret: (...args: unknown[]) => invoke('webhook_generate_secret', pack(args, [])),
    webhooksStatus: (...args: unknown[]) => invoke('webhook_status', pack(args, [])),
    getOrgTrigger: (...args: unknown[]) => invoke('org_get_trigger', pack(args, [])),
    setOrgTrigger: (...args: unknown[]) => invoke('org_set_trigger', pack(args, ["arg"])),
    listTriggerHistory: (...args: unknown[]) => invoke('triggers_list', pack(args, [])),
    decideTriggerHistory: (arg: { id: string; decision: 'approved' | 'rejected' }) =>
      invoke('trigger_history_decide', { arg }),
    clearTriggerHistory: (...args: unknown[]) => invoke('triggers_clear', pack(args, ["source"])),
    freeflowSetConfig: (...args: unknown[]) => invoke('freeflow_set_config', pack(args, ["patch"])),
    // Recorder hands {audio: ArrayBuffer, mimeType, filename, language}; the
    // backend wants base64 text.
    freeflowTranscribe: async (arg: Record<string, unknown>) => {
      try {
        const audio = arg.audio;
        const audioB64 =
          typeof arg.audioB64 === 'string' ? arg.audioB64
          : typeof audio === 'string' ? audio
          : audio instanceof ArrayBuffer || ArrayBuffer.isView(audio) ? toBase64(audio)
          : '';
        return await invoke('freeflow_transcribe', {
          audioB64,
          mime: arg.mime ?? arg.mimeType ?? null,
          filename: arg.filename ?? null,
          language: arg.language ?? null
        });
      } catch (e) {
        return { ok: false, error: errMsg(e) };
      }
    },
    integrationsList: (...args: unknown[]) => invoke('integrations_list', pack(args, [])),
    integrationsTemplates: (...args: unknown[]) => invoke('integrations_templates', pack(args, [])),
    integrationsUpsert: (...args: unknown[]) => invoke('integrations_upsert', pack(args, ["record"])),
    integrationsSetSecret: (...args: unknown[]) => invoke('integrations_set_secret', pack(args, ["payload"])),
    integrationsRemove: (...args: unknown[]) => invoke('integrations_remove', pack(args, ["payload"])),
    integrationsTest: (...args: unknown[]) => invoke('integrations_test', pack(args, ["payload"])),
    // Upstream shape: providerKeySet({backend, key}) — an OBJECT, not two
    // positional args. pack() would put the whole object under "provider" and
    // the backend's `provider: String` param rejects it ("invalid type: map").
    // Return envelope: backend returns a bare bool; upstream returns
    // {ok, error?} — wrap it so `r.ok` is never undefined on success.
    providerKeySet: async (req: { backend: string; key: string }) => {
      try {
        const v = await invoke('provider_key_set', { provider: req.backend, value: req.key });
        return v === true || (v as { ok?: boolean })?.ok === true
          ? { ok: true }
          : { ok: false, error: typeof v === 'string' ? v : 'rejected' };
      } catch (e) {
        return { ok: false, error: e instanceof Error ? e.message : String(e) };
      }
    },
    providerKeyHas: (backend: string) =>
      invoke('provider_key_has', { provider: backend }),
    providerKeyClear: (backend: string) =>
      invoke('provider_key_clear', { provider: backend }),
    // First-run quick start: one lightweight authenticated GET to the provider
    // to see whether a pasted key works — checked in memory, NOT stored. Resolves
    // {ok, code: 'valid'|'rejected'|'unreachable'|'unsupported'}; never rejects
    // for a bad key, so a failed check can never block onboarding.
    providerKeyValidate: async (provider: string, key: string) => {
      try {
        return await invoke<{ ok: boolean; code: string }>('provider_key_validate', { provider, key });
      } catch {
        return { ok: false, code: 'unreachable' };
      }
    },
    // First-run progress (engine_setup.rs). `engineSetupSnapshot` replays what a
    // window missed; `engineSetupRetry` re-runs the spawn ladder for an agent;
    // `hermesUserConfigStatus` is the read-only look at the user's own Hermes
    // setup ({exists, provider, model, hasKeys, path} — never a key value).
    engineSetupSnapshot: () => invoke('engine_setup_snapshot'),
    engineSetupRetry: (...args: unknown[]) => invoke('engine_setup_retry', pack(args, ["agentId"])),
    hermesUserConfigStatus: () => invoke('hermes_user_config_status'),
    realtimeHasOpenAiKey: (...args: unknown[]) => invoke('realtime_has_key', pack(args, [])),
    realtimeMintToken: (...args: unknown[]) => invoke('realtime_mint_token', pack(args, ["model"])),
    realtimeAction: (payload?: unknown) => invoke('realtime_action', { payload }),
    realtimeActionConfirm: (payload?: unknown) => invoke('realtime_action_confirm', { payload }),
    realtimeActionCancel: () => invoke('realtime_action_cancel'),
    realtimeSetSessionLive: (...args: unknown[]) => invoke('realtime_set_session_live', pack(args, ["live"])),
    realtimeDrainCompletions: (...args: unknown[]) => invoke('realtime_drain_completions', pack(args, [])),
    realtimeWaitFor: (...args: unknown[]) => invoke('realtime_wait_for', pack(args, ["taskIdOrAgent","timeoutMs"])),
    appInfo: (...args: unknown[]) => invoke('app_info', pack(args, [])),
    rosterRead: (...args: unknown[]) => invoke('roster_read', pack(args, [])),
    rosterReadSync: (): unknown => rosterCache,
    rosterWrite: (...args: unknown[]) =>
      invoke('roster_write', pack(args, ["snap"])).then((r) => {
        rosterCache = args[0];
        return r;
      }),
    updateCurrent: (...args: unknown[]) => invoke('update_current', pack(args, [])),
    updateRestartAndInstall: (...args: unknown[]) => invoke('update_restart_and_install', pack(args, [])),
    updateCheckNow: (...args: unknown[]) => invoke('update_check_now', pack(args, [])),
    updateDownload: (...args: unknown[]) => invoke('update_download', pack(args, [])),
    // Callers pass no url for "show me the releases page"; the backend requires
    // one (and refuses anything outside this app's repo), so fill the default in.
    updateOpenRelease: async (url?: string) => {
      try {
        return await invoke<boolean>('update_open_release', { url: url || RELEASES_URL });
      } catch (e) {
        console.warn('[cth] updateOpenRelease:', errMsg(e));
        return false;
      }
    },
    updateSimulate: (...args: unknown[]) => invoke('update_simulate', pack(args, ["version","notes"])),
    // ── events — upstream channel names verbatim (colon-style). ──────────
    onHiveMessage: (cb: (e: HiveRouteEvent) => void) => on<HiveRouteEvent>('hive:message', cb),
    onHiveDeliver: (cb: (e: unknown) => void) => on<unknown>('hive-deliver', cb),
    onHiveLog: (cb: (e: unknown) => void) => on<unknown>('hive-log', cb),
    onHiveContext: (cb: (e: unknown) => void) => on<unknown>('hive:contextUpdate', cb),
    onHiveContextUpdate: (cb: (e: { agentId: string; tokens: number; limit: number }) => void) =>
      on<{ agentId: string; tokens: number; limit: number }>('hive:contextUpdate', cb),
    // Per-id channels, exactly like upstream's `pty:data:<id>`/`pty:exit:<id>`
    // — the backend emits those alongside the aggregate events.
    // Window-scoped (onHere): each PTY's stream goes only to the window that
    // spawned it, so a floor's terminals never leak into another window.
    onPtyData: (id: string, cb: (data: string) => void) =>
      onHere<string>(`pty:data:${id}`, cb),
    onPtyExit: (
      id: string,
      cb: (info: { exitCode: number | null; signal: number | null }) => void
    ) => onHere<{ exitCode: number | null; signal: number | null }>(`pty:exit:${id}`, cb),
    onPtyRelaunch: (id: string, cb: () => void) => onHere<void>(`pty:relaunch:${id}`, () => cb()),
    onHookEvent: (cb: (e: unknown) => void) => on<unknown>('hive:hookEvent', cb),
    onHiveHookEvent: (cb: (e: unknown) => void) => on<unknown>('hive:hookEvent', cb),
    onAgentStatus: (cb: (e: unknown) => void) => on<unknown>('agent-status', cb),
    onAgentSpawned: (cb: (e: unknown) => void) => on<unknown>('hive:agentSpawned', cb),
    onHiveAgentSpawned: (cb: (e: unknown) => void) => on<unknown>('hive:agentSpawned', cb),
    onAgentArchived: (cb: (e: unknown) => void) => on<unknown>('hive:agentArchived', cb),
    onHiveAgentArchived: (cb: (e: { id: string }) => void) => on<{ id: string }>('hive:agentArchived', cb),
    onHiveDegraded: (cb: (e: { agentId?: string; name?: string; reason?: string; message?: string }) => void) =>
      on<{ agentId?: string; name?: string; reason?: string; message?: string }>('hive:degraded', cb),
    // Window-scoped like the PTY stream: the backend addresses a setup's events
    // to the window that owns the agent's terminal.
    onEngineSetup: (cb: (e: unknown) => void) => onHere<unknown>('engine:setup', cb),
    onBreakerState: (cb: (e: unknown) => void) => on<unknown>('control:breakerState', cb),
    onRealtimeCompletion: (cb: (e: unknown) => void) => on<unknown>('realtime:completion', cb),
    onUpdateState: (cb: (e: unknown) => void) => on<unknown>('update:status', cb),
    onUpdateStatus: (cb: (e: unknown) => void) => on<unknown>('update:status', cb),
    onTriggerHistory: (cb: (e: unknown) => void) => on<unknown>('triggerHistory:updated', cb),
    onMissionsUpdated: (cb: (e: unknown) => void) => on<unknown>('missions:updated', cb),
    onConfigChanged: (cb: (e: unknown) => void) => on<unknown>('config:changed', cb),
    // Window-scoped: main addresses it to itself; a floor must not show the
    // app-quit warning.
    onCloseRequested: (cb: (e: unknown) => void) => onHere<unknown>('app:closeRequested', cb),
    // A floor with live PTYs was asked to close — payload {ptyCount}.
    onFloorCloseRequested: (cb: (e: { ptyCount: number }) => void) =>
      onHere<{ ptyCount: number }>('floor:closeRequested', cb),
    onClosingTime: (cb: (e: unknown) => void) => on<unknown>('app:closingTime', cb),
    // Payload = the deep link's manifest source URL (string).
    onHirePending: (cb: (src: string) => void) => on<string>('hire-pending', cb),
    onPowerResume: (cb: (e: unknown) => void) => on<unknown>('power:resume', cb),
    onAutoCompact: (cb: () => void) => on<unknown>('mission:autoCompact', () => cb()),
    onContextTrigger: (cb: (e: { action: 'compact' | 'clear'; rule: unknown }) => void) =>
      on<{ action: 'compact' | 'clear'; rule: unknown }>('trigger:context', cb),
    onSlackMessage: (cb: (e: unknown) => void) => on<unknown>('slack:incomingMessage', cb),
    onTelemetryEvent: (cb: (e: unknown) => void) => on<unknown>('telemetry:event', cb),
    onRealtimeFloorDelta: (cb: (e: { text: string }) => void) => on<{ text: string }>('realtime:floorDelta', cb),
    onHiveTerminalHandoff: (cb: (e: unknown) => void) => on<unknown>('hive:terminalHandoff', cb),
    // Dead channel upstream too (no producer — legacy seam), but the hook
    // still registers the listener; map it so copied code never crashes.
    onHiveEnqueue: (cb: (e: { targetId: string; text: string }) => void) =>
      on<{ targetId: string; text: string }>('hive:enqueueToAgent', cb),
    // clear_context's commit path emits this — the renderer queue drains it.
    onRealtimeEnqueue: (cb: (e: { agentId: string; text: string }) => void) =>
      on<{ agentId: string; text: string }>('realtime:enqueue', cb),
  };
}
