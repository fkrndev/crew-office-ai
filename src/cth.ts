// GENERATED window.cth shim — upstream preload method→channel map.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';

const warned = new Set<string>();
const missing = (name: string) => (..._args: unknown[]) => {
  if (!warned.has(name)) { warned.add(name); console.warn('[cth] unimplemented:', name); }
  return Promise.resolve(null);
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

// Sync IPC — upstream's sendSync surface (clipboard + roster reads) rides a
// synchronous XHR against the loopback server (sync_api.rs), blocking the
// renderer thread exactly like Electron's sendSync. The caches remain as the
// pre-bind / failure fallback.
let clipboardCache = '';
let rosterCache: unknown = null;
let syncPort = 0;
const clipboardPoll = () => {
  void invoke<string | null>('app_clipboard_read').then((t) => {
    if (typeof t === 'string') clipboardCache = t;
  });
};
const rosterPoll = () => {
  void invoke<unknown>('roster_read').then((r) => {
    if (r && typeof r === 'object') rosterCache = r;
  });
};

/** sendSync — synchronous XHR against the loopback sync server. WebView2
 *  honours sync XHR (deprecated but functional); a null return means the
 *  server isn't bound yet — callers fall back to the warm caches. */
const syncGet = (path: string): string | null => {
  if (!syncPort) return null;
  try {
    const xhr = new XMLHttpRequest();
    xhr.open('GET', `http://127.0.0.1:${syncPort}${path}`, false);
    xhr.send();
    if (xhr.status === 200) return xhr.responseText;
  } catch {
    /* sync XHR rejected — cache fallback */
  }
  return null;
};

// Drag-drop file paths — with dragDropEnabled the OS drop never reaches the
// DOM in WebView2, so onDragDropEvent's paths are all we get. We re-emit a
// synthetic DOM `drop` carrying File objects whose Electron-style `.path` is
// filled in, so the copied renderer's drag handlers work unmodified.
let dropPaths: string[] = [];

export function installCth(): void {
  // Seed + refresh: focus refreshes are the cheap way to stay fresh without
  // an aggressive interval (a paste within ~2s of copy still lands).
  clipboardPoll();
  rosterPoll();
  void invoke<number>('sync_port').then((p) => {
    if (typeof p === 'number' && p > 0) syncPort = p;
  });
  window.setInterval(clipboardPoll, 500);
  window.setInterval(rosterPoll, 3000);
  window.addEventListener('focus', clipboardPoll);

  // Tauri's OS-level file drop carries the absolute paths WebView2 won't.
  // Re-emit it as a DOM `drop` on the element under the cursor — the copied
  // upstream drop handlers then read File.path exactly like Electron's.
  void getCurrentWebview()
    .onDragDropEvent((e) => {
      const p = e.payload;
      if (p.type !== 'drop' || p.paths.length === 0) return;
      dropPaths = [...p.paths];
      try {
        const dt = new DataTransfer();
        for (const path of p.paths) {
          const f = new File([], path.split(/[\\/]/).pop() ?? 'file');
          (f as File & { path: string }).path = path;
          dt.items.add(f);
        }
        const el =
          document.elementFromPoint(p.position.x, p.position.y) ?? document.body;
        el.dispatchEvent(
          new DragEvent('drop', {
            dataTransfer: dt,
            bubbles: true,
            cancelable: true,
            clientX: p.position.x,
            clientY: p.position.y,
          })
        );
      } catch {
        /* synthetic drop unsupported — pathForFile still has the queue */
      }
    })
    .catch(() => { /* drag events unsupported — pathForFile degrades */ });

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
    // Sync clipboard: cached (see clipboardPoll). Exact upstream signature.
    readClipboardSync: (): string => {
      const raw = syncGet('/sync/clipboard');
      if (raw != null) {
        try {
          const t = (JSON.parse(raw) as { text?: string }).text;
          if (typeof t === 'string') return (clipboardCache = t);
        } catch { /* fall through to cache */ }
      }
      return clipboardCache;
    },
    trackMessageSent: (...args: unknown[]) => invoke('analytics_message_sent', pack(args, ["event","props","Value>>"])),
    spawnPty: (...args: unknown[]) => invoke('spawn_agent', pack(args, ["args"])),
    writePty: (...args: unknown[]) => invoke('pty_write', pack(args, ["id","data"])),
    resizePty: (...args: unknown[]) => invoke('pty_resize', pack(args, ["id","cols","rows"])),
    redrawPty: (...args: unknown[]) => invoke('pty_redraw', pack(args, ["id"])),
    killPty: (...args: unknown[]) => invoke('kill_agent', pack(args, ["id"])),
    listPtys: (...args: unknown[]) => invoke('pty_list', pack(args, [])),
    resolveSessionCwd: (...args: unknown[]) => invoke('session_resolve_cwd', pack(args, ["agentId"])),
    chooseFolder: (...args: unknown[]) => invoke('dialog_choose_folder', pack(args, [])),
    openTerminalAt: (...args: unknown[]) => invoke('terminal_open_at_folder', pack(args, ["path"])),
    copyToClipboard: (...args: unknown[]) => invoke('app_clipboard_write', pack(args, ["text"])),
    readClipboard: (...args: unknown[]) => invoke('app_clipboard_read', pack(args, [])),
    getConfig: (...args: unknown[]) => invoke('get_config', pack(args, [])),
    updateConfig: (...args: unknown[]) => invoke('config_update', pack(args, ["patch"])),
    setAgentTokenCap: (...args: unknown[]) => invoke('config_set_agent_token_cap', pack(args, ["agentId","cap"])),
    setAgentCostCap: (...args: unknown[]) => invoke('config_set_agent_cost_cap', pack(args, ["agentId","cap"])),
    ensureHarnessHome: (...args: unknown[]) => invoke('config_ensure_home', pack(args, ["path"])),
    changeHome: (...args: unknown[]) => invoke('config_change_home', pack(args, ["newHome"])),
    listDir: (...args: unknown[]) => invoke('fs_list', pack(args, ["root","rel"])),
    readFile: (...args: unknown[]) => invoke('fs_read', pack(args, ["root","rel"])),
    readBinary: (...args: unknown[]) => invoke('fs_read_binary', pack(args, ["path"])),
    writeFile: (...args: unknown[]) => invoke('fs_write_file', pack(args, ["root","rel","content"])),
    statAbs: (...args: unknown[]) => invoke('fs_stat_abs', pack(args, ["path"])),
    revealPath: (...args: unknown[]) => invoke('fs_reveal_path', pack(args, ["path"])),
    gitIsRepo: (...args: unknown[]) => invoke('git_is_repo', pack(args, ["cwd"])),
    gitMainRepo: (...args: unknown[]) => invoke('git_main_repo', pack(args, ["cwd"])),
    gitBranch: (...args: unknown[]) => invoke('git_branch', pack(args, ["cwd"])),
    gitStatus: (...args: unknown[]) => invoke('git_status', pack(args, ["cwd"])),
    gitLog: (...args: unknown[]) => invoke('git_log', pack(args, ["cwd","n"])),
    gitBranches: (...args: unknown[]) => invoke('git_branches', pack(args, ["cwd"])),
    gitAheadBehind: (...args: unknown[]) => invoke('git_ahead_behind', pack(args, ["cwd"])),
    gitDiff: (...args: unknown[]) => invoke('git_diff', pack(args, ["cwd","relPath"])),
    gitLogGraph: (...args: unknown[]) => invoke('git_log_graph', pack(args, ["cwd","n","skip"])),
    gitCommitFiles: (...args: unknown[]) => invoke('git_commit_files', pack(args, ["cwd","sha"])),
    gitShowFile: (...args: unknown[]) => invoke('git_show_file', pack(args, ["cwd","rev","path"])),
    gitCompareRefs: (...args: unknown[]) => invoke('git_compare_refs', pack(args, ["cwd","base","head"])),
    gitWorktrees: (...args: unknown[]) => invoke('git_worktrees', pack(args, ["cwd"])),
    gitCheckout: (...args: unknown[]) => invoke('git_checkout', pack(args, ["cwd","r#ref","detach"])),
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
    toolsStatus: (...args: unknown[]) => invoke('tools_status', pack(args, [])),
    heroPayload: (...args: unknown[]) => invoke('hero_payload', pack(args, ["force"])),
    modelCatalog: (...args: unknown[]) => invoke('models_catalog', pack(args, [])),
    skillsLocal: (...args: unknown[]) => invoke('skills_local', pack(args, ["cwds"])),
    skillsCatalog: (...args: unknown[]) => invoke('skills_catalog', pack(args, ["force"])),
    skillsInstall: (...args: unknown[]) => invoke('skills_install', pack(args, ["url","name"])),
    skillsUninstall: (...args: unknown[]) => invoke('skills_uninstall', pack(args, ["path","cwds"])),
    skillsReveal: (...args: unknown[]) => invoke('skills_reveal', pack(args, ["id"])),
    searchMemory: (...args: unknown[]) => invoke('search_memory', pack(args, ["query","wing","limit"])),
    memoryWakeUp: (...args: unknown[]) => invoke('memory_wake_up', pack(args, ["wing"])),
    mineNow: (...args: unknown[]) => invoke('mine_now', pack(args, ["agentId"])),
    reflectNow: (...args: unknown[]) => invoke('reflect_now', pack(args, ["agentId"])),
    kgStatus: (...args: unknown[]) => invoke('kg_status', pack(args, [])),
    kgList: (...args: unknown[]) => invoke('kg_list', pack(args, [])),
    kgSearch: (...args: unknown[]) => invoke('kg_search', pack(args, ["query","limit"])),
    kgGet: (...args: unknown[]) => invoke('kg_get', pack(args, ["id"])),
    kgRemove: (...args: unknown[]) => invoke('kg_remove', pack(args, ["id"])),
    kgAddFiles: (...args: unknown[]) => invoke('kg_add_files', pack(args, ["payload"])),
    kgIngestFiles: (...args: unknown[]) => invoke('kg_ingest_files', pack(args, ["payload"])),
    attachFiles: (...args: unknown[]) => invoke('dialog_attach_files', pack(args, [])),
    saveClipboardImage: (...args: unknown[]) => invoke('clipboard_save_image', pack(args, ["base64"])),
    historyAdd: (...args: unknown[]) => invoke('command_history_add', pack(args, ["agentId","cwd","text"])),
    historyList: (...args: unknown[]) => invoke('command_history_list', pack(args, ["agentId","limit"])),
    historySearch: (...args: unknown[]) => invoke('command_history_search', pack(args, ["query","limit"])),
    hiveSend: (...args: unknown[]) => invoke('hive_send', pack(args, ["partial","from"])),
    drainPendingHires: (...args: unknown[]) => invoke('hire_drain_pending', pack(args, [])),
    importHireFiles: (...args: unknown[]) => invoke('hire_open_file', pack(args, [])),
    confirmClose: (...args: unknown[]) => invoke('app_confirm_close', pack(args, [])),
    cancelClose: (...args: unknown[]) => invoke('app_cancel_close', pack(args, [])),
    newFloor: (...args: unknown[]) => invoke('window_new_floor', pack(args, [])),
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
    controlAutoDelivery: (...args: unknown[]) => invoke('control_auto_delivery', pack(args, ["agentId","enabled"])),
    controlResume: (...args: unknown[]) => invoke('control_resume', pack(args, ["agentId"])),
    controlGateTool: (...args: unknown[]) => invoke('control_gate_tool', pack(args, ["agentId","tool","on"])),
    controlSteer: (...args: unknown[]) => invoke('control_steer', pack(args, ["agentId","text"])),
    controlHalt: (...args: unknown[]) => invoke('control_halt', pack(args, ["agentId"])),
    controlSnapshot: (...args: unknown[]) => invoke('control_snapshot', pack(args, ["agentId"])),
    hiveAddTask: (...args: unknown[]) => invoke('hive_add_task', pack(args, ["fields"])),
    hivePatchTask: (...args: unknown[]) => invoke('hive_patch_task', pack(args, ["id","patch"])),
    hiveDeleteTask: (...args: unknown[]) => invoke('hive_delete_task', pack(args, ["id"])),
    listMissions: (...args: unknown[]) => invoke('missions_list', pack(args, [])),
    saveMissions: (...args: unknown[]) => invoke('missions_save', pack(args, ["missions"])),
    textSearch: (...args: unknown[]) => invoke('hive_text_search', pack(args, ["query","limit"])),
    githubIssues: (...args: unknown[]) => invoke('github_issues', pack(args, ["repo"])),
    githubCIRuns: (...args: unknown[]) => invoke('github_ci_runs', pack(args, ["repo"])),
    setNotifications: (...args: unknown[]) => invoke('app_set_notifications', pack(args, ["enabled"])),
    openExternal: (...args: unknown[]) => invoke('app_open_external', pack(args, ["url"])),
    setLoginItem: (...args: unknown[]) => invoke('app_set_login_item', pack(args, ["enabled"])),
    hiveSetArchived: (...args: unknown[]) => invoke('hive_set_archived', pack(args, ["id","archived"])),
    slackStart: (...args: unknown[]) => invoke('slack_start', pack(args, ["port","channelId"])),
    slackStop: (...args: unknown[]) => invoke('slack_stop', pack(args, [])),
    slackStatus: (...args: unknown[]) => invoke('slack_status', pack(args, [])),
    slackReply: (...args: unknown[]) => invoke('slack_reply', pack(args, ["channel","threadTs","text"])),
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
    freeflowTranscribe: (...args: unknown[]) => invoke('freeflow_transcribe', pack(args, ["audioB64","mime","filename","language"])),
    integrationsList: (...args: unknown[]) => invoke('integrations_list', pack(args, [])),
    integrationsTemplates: (...args: unknown[]) => invoke('integrations_templates', pack(args, [])),
    integrationsUpsert: (...args: unknown[]) => invoke('integrations_upsert', pack(args, ["record"])),
    integrationsSetSecret: (...args: unknown[]) => invoke('integrations_set_secret', pack(args, ["payload"])),
    integrationsRemove: (...args: unknown[]) => invoke('integrations_remove', pack(args, ["payload"])),
    integrationsTest: (...args: unknown[]) => invoke('integrations_test', pack(args, ["payload"])),
    // Upstream shape: providerKeySet({backend, key}) — an OBJECT, not two
    // positional args. pack() would put the whole object under "provider" and
    // the backend's `provider: String` param rejects it ("invalid type: map").
    providerKeySet: (req: { backend: string; key: string }) =>
      invoke('provider_key_set', { provider: req.backend, value: req.key }),
    providerKeyHas: (backend: string) =>
      invoke('provider_key_has', { provider: backend }),
    providerKeyClear: (backend: string) =>
      invoke('provider_key_clear', { provider: backend }),
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
    rosterReadSync: (): unknown => {
      const raw = syncGet('/sync/roster');
      if (raw != null) {
        try {
          const v = JSON.parse(raw) as unknown;
          if (v != null) return (rosterCache = v);
        } catch { /* fall through to cache */ }
      }
      return rosterCache;
    },
    rosterWrite: (...args: unknown[]) =>
      invoke('roster_write', pack(args, ["snap"])).then((r) => {
        rosterCache = args[0];
        return r;
      }),
    updateCurrent: (...args: unknown[]) => invoke('update_current', pack(args, [])),
    updateRestartAndInstall: (...args: unknown[]) => invoke('update_restart_and_install', pack(args, [])),
    updateCheckNow: (...args: unknown[]) => invoke('update_check_now', pack(args, [])),
    updateDownload: (...args: unknown[]) => invoke('update_download', pack(args, [])),
    updateOpenRelease: (...args: unknown[]) => invoke('update_open_release', pack(args, ["url"])),
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
    onPtyData: (id: string, cb: (data: string) => void) =>
      on<string>(`pty:data:${id}`, cb),
    onPtyExit: (
      id: string,
      cb: (info: { exitCode: number | null; signal: number | null }) => void
    ) => on<{ exitCode: number | null; signal: number | null }>(`pty:exit:${id}`, cb),
    onPtyRelaunch: (id: string, cb: () => void) => on<void>(`pty:relaunch:${id}`, () => cb()),
    onHookEvent: (cb: (e: unknown) => void) => on<unknown>('hive:hookEvent', cb),
    onHiveHookEvent: (cb: (e: unknown) => void) => on<unknown>('hive:hookEvent', cb),
    onAgentStatus: (cb: (e: unknown) => void) => on<unknown>('agent-status', cb),
    onAgentSpawned: (cb: (e: unknown) => void) => on<unknown>('hive:agentSpawned', cb),
    onHiveAgentSpawned: (cb: (e: unknown) => void) => on<unknown>('hive:agentSpawned', cb),
    onAgentArchived: (cb: (e: unknown) => void) => on<unknown>('hive:agentArchived', cb),
    onHiveAgentArchived: (cb: (e: { id: string }) => void) => on<{ id: string }>('hive:agentArchived', cb),
    onBreakerState: (cb: (e: unknown) => void) => on<unknown>('control:breakerState', cb),
    onRealtimeCompletion: (cb: (e: unknown) => void) => on<unknown>('realtime:completion', cb),
    onUpdateState: (cb: (e: unknown) => void) => on<unknown>('update:status', cb),
    onUpdateStatus: (cb: (e: unknown) => void) => on<unknown>('update:status', cb),
    onTriggerHistory: (cb: (e: unknown) => void) => on<unknown>('triggerHistory:updated', cb),
    onMissionsUpdated: (cb: (e: unknown) => void) => on<unknown>('missions:updated', cb),
    onConfigChanged: (cb: (e: unknown) => void) => on<unknown>('config:changed', cb),
    onCloseRequested: (cb: (e: unknown) => void) => on<unknown>('app:closeRequested', cb),
    onClosingTime: (cb: (e: unknown) => void) => on<unknown>('app:closingTime', cb),
    onHirePending: (cb: (e: unknown) => void) => on<unknown>('hire-pending', cb),
    onHireImport: (cb: (e: unknown) => void) => on<unknown>('hire:import', cb),
    onHireError: (cb: (e: { error: string }) => void) => on<{ error: string }>('hire:error', cb),
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
