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

export const hiveRegistry = () => invoke<RegistryEntry[]>('hive_registry');
export const hiveSend = (from: string, to: string, act: string, subject: string, body: string) =>
  invoke<Message>('hive_send', { from, to, act, subject, body });
export const hiveInbox = (agentId: string) => invoke<Message[]>('hive_inbox', { agentId });
export const hiveMarkDone = (agentId: string, msgId: string) =>
  invoke('hive_mark_done', { agentId, msgId });
export const hiveBoard = () => invoke<string>('hive_board');
export const readMemory = (agentId: string) => invoke<string>('read_memory', { agentId });
export const appendMemory = (agentId: string, entry: string) =>
  invoke('append_memory', { agentId, entry });
export const getConfig = () => invoke<Config>('get_config');
export const setAutoMode = (enabled: boolean) => invoke('set_auto_mode', { enabled });

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
