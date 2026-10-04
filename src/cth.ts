// The `window.cth` surface upstream preload exposes (contextBridge name
// `cth`). The scene/renderer code calls it verbatim; each method maps onto
// the Tauri command surface — channel names keep upstream's `hive:message`
// style because Tauri event names allow ':'.
import { invoke } from '@tauri-apps/api/core';
import { listen, type EventCallback } from '@tauri-apps/api/event';

// HiveRouteEvent is declared ambient in vite-env.d.ts (the upstream preload
// contract) — act carries the MessageAct union.

// The Window.cth type lives in ./cth.d.ts (ambient, no module wrapper) so
// copied upstream renderer code compiles verbatim.

function on<T>(channel: string, cb: (e: T) => void): () => void {
  const unlisten = listen(channel, ((ev: { payload: T }) => cb(ev.payload)) as EventCallback<T>);
  return () => {
    void unlisten.then((u) => u());
  };
}

export function installCth(): void {
  window.cth = {
    hiveTasks: () => invoke('hive_tasks'),
    hiveBoard: () => invoke('hive_board'),
    hiveRegistry: () => invoke('hive_registry'),
    hiveLog: (n?: number) => invoke('hive_log', { n }),
    hiveSend: (partial: unknown, from?: string) => invoke('hive_send', { partial, from }),
    hiveMessages: (opts?: unknown) => invoke('hive_messages', { opts }),
    hiveInbox: (id: string) => invoke('hive_inbox', { id }),
    hiveAgentDirectory: () => invoke('hive_agent_directory'),
    onHiveMessage: (cb: (e: HiveRouteEvent) => void) => on<HiveRouteEvent>('hive:message', cb),
    onHiveDeliver: (cb: (e: unknown) => void) => on<unknown>('hive-deliver', cb),
    onPtyData: (id: string, cb: (data: string) => void) =>
      on<{ id: string; data: string }>('pty-output', (e) => {
        if (e.id === id) cb(e.data);
      }),
    onPtyExit: (id: string, cb: (info: unknown) => void) =>
      on<{ id: string; code: number | null }>('pty-exit', (e) => {
        if (e.id === id) cb(e);
      }),
    onHookEvent: (cb: (e: unknown) => void) => on<unknown>('hook-event', cb),
    onAgentStatus: (cb: (e: unknown) => void) => on<unknown>('agent-status', cb)
  };
}
