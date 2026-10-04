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
    hiveTasks(): Promise<{ tasks?: any[] } | null>;
    hiveBoard(): Promise<any>;
    hiveRegistry(): Promise<{ godId?: string; agents: Record<string, any> }>;
    /** `Parameters<typeof hiveAddTask>[0]` is the card type upstream code
     *  re-derives for task creation — the loose record keeps it any-friendly. */
    hiveAddTask(fields: Record<string, any>): Promise<any>;
    hiveLog(n?: number): Promise<any>;
    hiveSend(partial: unknown, from?: string): Promise<any>;
    hiveMessages(opts?: unknown): Promise<any>;
    hiveInbox(id: string): Promise<any>;
    hiveAgentDirectory(): Promise<any>;
    onHiveMessage(cb: (e: HiveRouteEvent) => void): () => void;
    onHiveDeliver(cb: (e: any) => void): () => void;
    onPtyData(id: string, cb: (data: string) => void): () => void;
    onPtyExit(
      id: string,
      cb: (info: { exitCode: number | null; signal: number | null }) => void
    ): () => void;
    onHookEvent(cb: (e: any) => void): () => void;
    onAgentStatus(cb: (e: any) => void): () => void;
    // Upstream's preload exposes ~200 members; the shim (cth.ts) implements
    // them all. `any` keeps copied code compiling verbatim — method-level
    // types above are just the ones the Pixi floor consumes.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    [key: string]: any;
  }

  interface Window {
    cth: CthApi;
  }
}

export {};
