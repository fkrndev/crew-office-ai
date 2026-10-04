import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

// ─── types mirroring the Rust side ──────────────────────────────────────────

export interface RegistryEntry {
  id: string;
  name: string;
  provider: string;
  role: string;
  cwd: string;
  is_god: boolean;
  status: string;
  created_at: string;
}

export interface Message {
  id: string;
  conversation: string;
  in_reply_to: string | null;
  from: string;
  to: string;
  act: string;
  subject: string;
  body: string;
  hops: number;
  requires_reply: boolean;
  needs_human: boolean;
  created_at: string;
}

export interface Config {
  auto_mode: boolean;
  god_name: string;
}

export interface SpawnArgs {
  provider: string;
  name: string;
  role?: string;
  cwd: string;
  command: string;
  is_god?: boolean;
  cols?: number;
  rows?: number;
}

export interface PtyOutputPayload {
  id: string;
  data: string;
}
export interface PtyExitPayload {
  id: string;
  code: number | null;
}
export interface Delivery {
  from: string;
  to: string;
  msg_id: string;
  bounced: boolean;
}

// ─── commands ───────────────────────────────────────────────────────────────

export const spawnAgent = (args: SpawnArgs) => invoke<{ id: string }>('spawn_agent', { args });
export const ptyWrite = (id: string, data: string) => invoke('pty_write', { id, data });
export const ptyResize = (id: string, cols: number, rows: number) =>
  invoke('pty_resize', { id, cols, rows });
export const killAgent = (id: string) => invoke('kill_agent', { id });
export const respawnAgent = (agentId: string) => invoke<{ id: string }>('respawn_agent', { agentId });

export interface HiveRegistryDoc {
  godId: string | null;
  agents: Record<string, RegistryEntry>;
}
export const hiveRegistry = () => invoke<HiveRegistryDoc>('hive_registry');
export const hiveSend = (
  from: string,
  to: string,
  act: string,
  subject: string,
  body: string,
  extra?: { conversation?: string; in_reply_to?: string; requires_reply?: boolean; needs_human?: boolean }
) =>
  invoke<{ ok: boolean; message: Message }>('hive_send', {
    partial: { to, act, subject, body, ...extra },
    from
  });
export const hiveInbox = (agentId: string) => invoke<Message[]>('hive_inbox', { agentId });
export const hiveMarkDone = (agentId: string, msgId: string) =>
  invoke('hive_mark_done', { agentId, msgId });
export const hiveBoard = () => invoke<string>('hive_board');
export const readMemory = (agentId: string) => invoke<string>('read_memory', { agentId });
export const appendMemory = (agentId: string, entry: string) =>
  invoke('append_memory', { agentId, entry });
export const getConfig = () => invoke<Config>('get_config');
export const setAutoMode = (enabled: boolean) => invoke('set_auto_mode', { enabled });

// ─── control + breaker ──────────────────────────────────────────────────────

export interface ControlSnapshot {
  paused: boolean;
  halted: boolean;
  autoDeliveryPaused: boolean;
  gatedTools: string[];
  pendingSteers: number;
}

export const controlPause = (agentId: string, on: boolean) =>
  invoke<ControlSnapshot>('control_pause', { agentId, on });
export const controlResume = (agentId: string) =>
  invoke<ControlSnapshot>('control_resume', { agentId });
export const controlHalt = (agentId: string) =>
  invoke<ControlSnapshot>('control_halt', { agentId });
export const controlSteer = (agentId: string, text: string) =>
  invoke<ControlSnapshot>('control_steer', { agentId, text });
export const controlGateTool = (agentId: string, tool: string, on: boolean) =>
  invoke<ControlSnapshot>('control_gate_tool', { agentId, tool, on });
export const controlSnapshot = (agentId: string) =>
  invoke<ControlSnapshot>('control_snapshot', { agentId });

export interface BreakerStatePayload {
  agent_id: string;
  level: 'healthy' | 'steering' | 'constrained' | 'stopped';
  reason: string;
  ts: number;
}
export const onBreakerState = (cb: (p: BreakerStatePayload) => void): Promise<UnlistenFn> =>
  listen<BreakerStatePayload>('breaker-state', (e) => cb(e.payload));
export const onBreakerToast = (cb: (p: { text: string; reason: string }) => void): Promise<UnlistenFn> =>
  listen<{ text: string; reason: string }>('breaker-toast', (e) => cb(e.payload));

// ─── events ─────────────────────────────────────────────────────────────────

export const onPtyOutput = (cb: (p: PtyOutputPayload) => void): Promise<UnlistenFn> =>
  listen<PtyOutputPayload>('pty-output', (e) => cb(e.payload));
export const onPtyExit = (cb: (p: PtyExitPayload) => void): Promise<UnlistenFn> =>
  listen<PtyExitPayload>('pty-exit', (e) => cb(e.payload));
export const onDeliver = (cb: (d: Delivery) => void): Promise<UnlistenFn> =>
  listen<Delivery>('hive-deliver', (e) => cb(e.payload));
export interface AgentStatusPayload {
  id: string;
  status: string;
  detail?: string;
}
export const onAgentStatus = (cb: (p: AgentStatusPayload) => void): Promise<UnlistenFn> =>
  listen<AgentStatusPayload>('agent-status', (e) => cb(e.payload));

// ─── hive admin / tasks / agents ────────────────────────────────────────────
export const hiveAddTask = (fields: Record<string, unknown>) => invoke('hive_add_task', { fields });
export const hiveDeleteTask = (id: string) => invoke('hive_delete_task', { id });
export const hivePatchTask = (id: string, patch: Record<string, unknown>) =>
  invoke('hive_patch_task', { id, patch });
export const hivePatchAgentRole = (id: string, role: string) =>
  invoke('hive_patch_agent_role', { id, role });
export const hiveRenameAgent = (id: string, name: string) => invoke('hive_rename_agent', { id, name });
export const hiveSetArchived = (id: string, archived: boolean) =>
  invoke('hive_set_archived', { id, archived });
export const hiveSetAgentHold = (id: string, hold: boolean) => invoke('hive_set_agent_hold', { id, hold });
export const hiveAgentDirectory = (id?: string) => invoke('hive_agent_directory', id ? { id } : {});
export const hiveAgentUsage = (id: string) => invoke('hive_agent_usage', { id });
export const hiveAgentContext = (id: string) => invoke('hive_agent_context', { id });
export const hiveLog = (limit?: number) => invoke('hive_log', { limit });
export const hiveMessages = (id: string, limit?: number) => invoke('hive_messages', { agentId: id, limit });
export const hiveTextSearch = (q: string, limit?: number) => invoke('hive_text_search', { q, limit });
export const sessionResolveCwd = (agentId: string) => invoke('session_resolve_cwd', { agentId });
export const ptyList = () => invoke<string[]>('pty_list');
export const workersList = () => invoke('workers_list');
export const workersStop = (id: string) => invoke('workers_stop', { id });

// ─── git pane ───────────────────────────────────────────────────────────────
const git = (name: string, args: Record<string, unknown>) => invoke(name, args);
export const gitIsRepo = (cwd: string) => git('git_is_repo', { cwd });
export const gitBranch = (cwd: string) => git('git_branch', { cwd });
export const gitStatus = (cwd: string) => git('git_status', { cwd });
export const gitLog = (cwd: string, n?: number) => git('git_log', { cwd, n });
export const gitLogGraph = (cwd: string, n?: number, skip?: number) => git('git_log_graph', { cwd, n, skip });
export const gitBranches = (cwd: string) => git('git_branches', { cwd });
export const gitAheadBehind = (cwd: string) => git('git_ahead_behind', { cwd });
export const gitMainRepo = (cwd: string) => git('git_main_repo', { cwd });
// Upstream git:diff = working-tree diff of ONE file; rev-range diff is git:diffRefs.
export const gitDiff = (cwd: string, relPath: string) => git('git_diff', { cwd, relPath });
export const gitDiffRefs = (cwd: string, revA: string, revB: string, path?: string) =>
  git('git_diff_refs', { cwd, revA, revB, path });
export const gitCheckout = (cwd: string, ref: string, detach?: boolean) =>
  git('git_checkout', { cwd, ref, detach });
export const gitWorktrees = (cwd: string) => git('git_worktrees', { cwd });
export const gitWorktreeAdd = (cwd: string, path: string, branch?: string, baseRev?: string) =>
  git('git_worktree_add', { cwd, path, branch, baseRev });
export const gitWorktreeRemove = (path: string) => git('git_worktree_remove', { path });
export const gitCommitFiles = (cwd: string, sha: string) => git('git_commit_files', { cwd, sha });
export const gitCompareRefs = (cwd: string, base: string, head: string) =>
  git('git_compare_refs', { cwd, base, head });
export const gitShowFile = (cwd: string, rev: string, path: string) => git('git_show_file', { cwd, rev, path });
export const gitSnapshot = (cwd: string) => git('git_snapshot', { cwd });

// ─── platform / misc ────────────────────────────────────────────────────────
export const appInfo = () => invoke('app_info');
export const appOpenExternal = (url: string) => invoke('app_open_external', { url });
export const appClipboardWrite = (text: string) => invoke('app_clipboard_write', { text });
export const appClipboardRead = () => invoke<string>('app_clipboard_read');
export const appResetAll = () => invoke('app_reset_all');
export const appSetLoginItem = (enabled: boolean) => invoke('app_set_login_item', { enabled });
export const appSetNotifications = (enabled: boolean) => invoke('app_set_notifications', { enabled });
export const appConfirmClose = () => invoke('app_confirm_close');
export const appCancelClose = () => invoke('app_cancel_close');
export const terminalOpenAtFolder = (path: string) => invoke('terminal_open_at_folder', { path });
export const toolsStatus = () => invoke('tools_status');
export const fsStatAbs = (path: string) => invoke('fs_stat_abs', { path });
export const fsReadBinary = (path: string) => invoke('fs_read_binary', { path });
export const fsWriteFile = (root: string, rel: string, content: string) =>
  invoke('fs_write_file', { root, rel, content });
export const fsRevealPath = (path: string) => invoke('fs_reveal_path', { path });
export const dialogChooseFolder = () => invoke('dialog_choose_folder');
export const dialogAttachFiles = () => invoke('dialog_attach_files');
export const modelsCatalog = (force?: boolean) => invoke('models_catalog', { force });
export const heroPayload = (force?: boolean) => invoke('hero_payload', { force });
export const windowNewFloor = () => invoke('window_new_floor');
export const configChangeHome = (newHome: string) => invoke('config_change_home', { newHome });
export const configEnsureHome = (path: string) => invoke('config_ensure_home', { path });
export const configHomeSync = () => invoke<string>('config_home_sync');
export const configUpdate = (patch: Record<string, unknown>) => invoke('config_update', { patch });
export const configSetAgentTokenCap = (agentId: string, cap: number | null) =>
  invoke('config_set_agent_token_cap', { agentId, cap });
export const rosterReadSync = () => invoke('roster_read_sync');
export const rosterRead = () => invoke('roster_read');
export const rosterWrite = (snap: Record<string, unknown>) => invoke('roster_write', { snap });
export const rosterArchive = () => invoke('roster_archive');

// ─── secrets / provider keys ────────────────────────────────────────────────
export const providerKeySet = (provider: string, value: string) =>
  invoke('provider_key_set', { provider, value });
export const providerKeyHas = (provider: string) => invoke<boolean>('provider_key_has', { provider });
export const providerKeyClear = (provider: string) => invoke('provider_key_clear', { provider });
export const secretsList = () => invoke<string[]>('secrets_list');

// ─── webhooks / slack / triggers ────────────────────────────────────────────
export const webhookStart = (port?: number) => invoke('webhook_start', { port });
export const webhookStop = () => invoke('webhook_stop');
export const webhookStatus = () => invoke('webhook_status');
export const webhookGenerateSecret = () => invoke<string>('webhook_generate_secret');
export const webhookSetConfig = (port?: number, enabled?: boolean) =>
  invoke('webhook_set_config', { port, enabled });
export const webhooksList = () => invoke('webhooks_list');
export const webhooksSave = (endpoints: unknown[]) => invoke('webhooks_save', { endpoints });
export const webhooksDelete = (id: string) => invoke('webhooks_delete', { id });
export const webhookPending = () => invoke('webhook_pending');
export const slackStart = (port: number, channelId?: string) => invoke('slack_start', { port, channelId });
export const slackStop = () => invoke('slack_stop');
export const slackStatus = () => invoke('slack_status');
export const slackSetConfig = (cfg: Record<string, unknown>) => invoke('slack_set_config', { cfg });
export const slackReply = (channel: string, threadTs: string, text: string) =>
  invoke('slack_reply', { channel, threadTs, text });
export const slackReplyScriptPath = () => invoke<string>('slack_reply_script_path');
export const triggersList = () => invoke('triggers_list');
export const triggersGetContext = () => invoke('triggers_get_context');
export const triggersSetContext = (ctx: Record<string, unknown>) => invoke('triggers_set_context', { ctx });
export const orgGetTrigger = () => invoke('org_get_trigger');
export const orgSetTrigger = (arg: Record<string, unknown>) => invoke('org_set_trigger', { arg });
export const controlSetBreakerState = (agentId: string, enabled: boolean) =>
  invoke('control_set_breaker_state', { agentId, enabled });
// Upstream polarity is `paused` — the command takes the positive `enabled`.
export const controlAutoDelivery = (agentId: string, paused: boolean) =>
  invoke('control_auto_delivery', { agentId, enabled: !paused });

// ─── telemetry / cost / memory ──────────────────────────────────────────────
export const telemetrySnapshot = () => invoke('telemetry_snapshot');
export const telemetrySpans = (agentId: string) => invoke('telemetry_spans', { agentId });
export const telemetryUsage = () => invoke('telemetry_usage');
export const costLifetime = () => invoke('cost_lifetime');
export const memoryStatus = () => invoke('memory_status');
export const searchMemory = (query: string, wing?: string, limit?: number) =>
  invoke('search_memory', { query, wing, limit });
export const memoryWakeUp = (wing?: string) => invoke('memory_wake_up', { wing });
export const mineNow = (agentId?: string) => invoke('mine_now', { agentId });
export const reflectNow = (agentId?: string) => invoke('reflect_now', { agentId });

// ─── skills / hire / missions / history ─────────────────────────────────────
export const skillsLocal = (cwds: string[]) => invoke('skills_local', { cwds });
export const skillsCatalog = (force?: boolean) => invoke('skills_catalog', { force });
export const skillsInstall = (url: string, name: string) => invoke('skills_install', { url, name });
export const skillsUninstall = (path: string, cwds: string[]) =>
  invoke('skills_uninstall', { path, cwds });
export const skillsReveal = (id: string) => invoke('skills_reveal', { id });
export const hireValidate = (url: string) => invoke('hire_validate', { url });
export const hireFetch = (url: string) => invoke('hire_fetch', { url });
export const hireImportFile = (path: string) => invoke('hire_import_file', { path });
export const hireOpenFile = () => invoke('hire_open_file');
export const hireDrainPending = () => invoke<string | null>('hire_drain_pending');
export const missionsList = () => invoke('missions_list');
export const kvGet = (key: string) => invoke('kv_get', { key });
export const kvSet = (key: string, value: unknown) => invoke('kv_set', { key, value });
export const commandHistoryAdd = (agentId: string, cwd: string | null, text: string) =>
  invoke('command_history_add', { agentId, cwd, text });
export const commandHistoryList = (agentId?: string, limit?: number) =>
  invoke('command_history_list', { agentId, limit });
export const commandHistorySearch = (query: string, limit?: number) =>
  invoke('command_history_search', { query, limit });

// ─── kg / integrations / adapters ───────────────────────────────────────────
export const kgStatus = () => invoke('kg_status');
export const kgList = () => invoke('kg_list');
export const kgSearch = (query: string, limit?: number) => invoke('kg_search', { query, limit });
export const kgGet = (id: string) => invoke('kg_get', { id });
export const kgRemove = (id: string) => invoke('kg_remove', { id });
export const kgIngestFiles = (payload: Record<string, unknown>) => invoke('kg_ingest_files', { payload });
export const kgAddFiles = (payload: Record<string, unknown>) => invoke('kg_add_files', { payload });
export const integrationsList = () => invoke('integrations_list');
export const integrationsTemplates = () => invoke('integrations_templates');
export const integrationsUpsert = (record: Record<string, unknown>) =>
  invoke('integrations_upsert', { record });
export const integrationsRemove = (payload: { id: string }) => invoke('integrations_remove', { payload });
export const integrationsSetSecret = (payload: Record<string, unknown>) =>
  invoke('integrations_set_secret', { payload });
export const integrationsTest = (payload: Record<string, unknown>) =>
  invoke('integrations_test', { payload });
export const integrationsBrokerStart = (preferredPort?: number) =>
  invoke('integrations_broker_start', { preferredPort });
export const integrationsBrokerStop = () => invoke('integrations_broker_stop');
export const integrationsBrokerStatus = () => invoke('integrations_broker_status');
export const integrationsBrokerToken = (payload: Record<string, unknown>) =>
  invoke('integrations_broker_token', { payload });
export const githubCiRuns = (repo?: string) => invoke('github_ci_runs', { repo });
export const updateCheckNow = () => invoke('update_check_now');
export const updateCurrent = () => invoke('update_current');
export const updateOpenRelease = (url: string) => invoke('update_open_release', { url });
export const updateDownload = () => invoke('update_download');
export const updateRestartAndInstall = () => invoke('update_restart_and_install');
export const updateSimulate = (version?: string, notes?: string) =>
  invoke('update_simulate', { version, notes });
export const githubIssues = (repo?: string) => invoke('github_issues', { repo });
export const githubSetRepo = (ownerRepo: string) => invoke('github_set_repo', { ownerRepo });
export const freeflowSetConfig = (patch: Record<string, unknown>) => invoke('freeflow_set_config', { patch });
export const freeflowTranscribe = (audioB64: string, mime: string) =>
  invoke('freeflow_transcribe', { audioB64, mime });
export const freeflowStatus = () => invoke('freeflow_status');
export const analyticsMessageSent = (event: string, props?: Record<string, unknown>) =>
  invoke('analytics_message_sent', { event, props });
export const analyticsSetOptIn = (on: boolean) => invoke('analytics_set_opt_in', { on });
export const analyticsStatus = () => invoke('analytics_status');
export const realtimeSetSessionLive = (live: boolean) => invoke('realtime_set_session_live', { live });
export const realtimeHasKey = () => invoke<boolean>('realtime_has_key');
export const realtimeMintToken = (model?: string) =>
  invoke<{ ok: boolean; token?: string; expiresAt?: number | null; sessionConfig?: { model: string }; error?: string; code?: string }>(
    'realtime_mint_token',
    { model },
  );
export const realtimeWaitFor = (taskIdOrAgent: string, timeoutMs?: number) =>
  invoke('realtime_wait_for', { taskIdOrAgent, timeoutMs });
export const realtimeDrainCompletions = () => invoke('realtime_drain_completions');
export const realtimeTrackDispatch = (dispatch: Record<string, unknown>) =>
  invoke('realtime_track_dispatch', { dispatch });
export const realtimeActionsStatus = () => invoke('realtime_actions_status');
export const closingTimeStart = () => invoke('closing_time_start');
export const closingTimeCancel = () => invoke('closing_time_cancel');
export const closingTimeStatus = () => invoke('closing_time_status');
export const breakerLevel = (agentId: string) => invoke('breaker_level', { agentId });
export const onClosing = (cb: (p: string) => void): Promise<UnlistenFn> =>
  listen<string>('closing', (e) => cb(e.payload));
export const onCloseRequested = (cb: () => void): Promise<UnlistenFn> =>
  listen('close-requested', () => cb());
