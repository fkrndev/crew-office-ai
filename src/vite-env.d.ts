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
    hiveMessages(...args: any[]): Promise<any>;
    hiveInbox(id: string): Promise<any>;
    hiveAgentDirectory(): Promise<any>;
    /** `conversation_read`: clean user/assistant turns from the engine's own
     *  transcript, incremental via the opaque cursor (simple-mode chat view). */
    conversationRead(agentId: string, sinceCursor?: string | null): Promise<import('@shared/conversation').ConversationRead>;
    /** Kills every PTY then exits the app (`app_force_quit`). */
    forceQuit(): Promise<boolean>;
    /** Graceful shutdown — resolves the live worker count; rejects on failure. */
    startClosingTime(): Promise<number>;
    /** `hire_drain_pending`: the parked deep-link manifest URL, or null. */
    drainPendingHires(): Promise<string | null>;
    /** `hire_fetch`: resolve a manifest URL into a validated manifest. */
    hireFetch(url: string): Promise<{ ok: boolean; manifest?: any; consentRequired?: string[] }>;
    /** Deep link arrived while running; payload is the manifest URL. */
    onHirePending(cb: (src: string) => void): () => void;
    /** `clipboard_image_to_file`: write a pasted PNG (base64 / data URL) to a temp
     *  file and resolve its absolute path. */
    clipboardImageToFile(base64: string): Promise<{ ok: boolean; path?: string; error?: string }>;
    /** Cached harness-home read (null until the first async prime lands). */
    harnessHomeSync(): string | null;
    onHiveMessage(cb: (e: HiveRouteEvent) => void): () => void;
    onHiveDeliver(cb: (e: any) => void): () => void;
    onPtyData(id: string, cb: (data: string) => void): () => void;
    onPtyExit(
      id: string,
      cb: (info: { exitCode: number | null; signal: number | null }) => void
    ): () => void;
    onHookEvent(cb: (e: any) => void): () => void;
    /** `engine:setup` — first-run install/start progress (shared/engineSetup.ts). */
    onEngineSetup(cb: (e: any) => void): () => void;
    engineSetupSnapshot(): Promise<any[]>;
    engineSetupRetry(agentId: string): Promise<boolean>;
    /** Read-only look at the user's own Hermes setup — never a key value. */
    hermesUserConfigStatus(): Promise<{
      exists: boolean; provider: string | null; model: string | null; hasKeys: boolean; path: string;
    }>;
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
