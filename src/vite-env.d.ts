/// <reference types="vite/client" />

/// `window.cth` — the upstream preload bridge contract (contextBridge name
/// `cth`), declared ambient so copied renderer code compiles verbatim.
/// Runtime installation lives in ./cth.ts.
declare global {
  interface HiveRouteEvent {
    id: string;
    from: string;
    to: string;
    act: 'request' | 'inform' | 'propose' | 'query' | 'agree' | 'refuse' | 'done';
    subject: string;
    targets: string[];
    needsHuman: boolean;
  }

  interface CthApi {
    hiveTasks(): Promise<{ tasks?: unknown[] } | null>;
    hiveBoard(): Promise<string>;
    hiveRegistry(): Promise<unknown>;
    hiveLog(n?: number): Promise<unknown>;
    hiveSend(partial: unknown, from?: string): Promise<unknown>;
    hiveMessages(opts?: unknown): Promise<unknown>;
    hiveInbox(id: string): Promise<unknown>;
    hiveAgentDirectory(): Promise<unknown>;
    onHiveMessage(cb: (e: HiveRouteEvent) => void): () => void;
    onHiveDeliver(cb: (e: unknown) => void): () => void;
    onPtyData(id: string, cb: (data: string) => void): () => void;
    onPtyExit(id: string, cb: (info: unknown) => void): () => void;
    onHookEvent(cb: (e: unknown) => void): () => void;
    onAgentStatus(cb: (e: unknown) => void): () => void;
    [key: string]: unknown;
  }

  interface Window {
    cth: CthApi;
  }
}

export {};
