import { create } from 'zustand';
import type { BreakerStatePayload, Message, RegistryEntry } from './api';
import { hiveRegistry } from './api';

interface AppState {
  agents: RegistryEntry[];
  selected: string | null;
  inbox: Record<string, Message[]>;
  breaker: Record<string, BreakerStatePayload>;
  refreshAgents: () => Promise<void>;
  select: (id: string | null) => void;
  setInbox: (agentId: string, msgs: Message[]) => void;
  setBreaker: (s: BreakerStatePayload) => void;
}

export const useApp = create<AppState>((set) => ({
  agents: [],
  selected: null,
  inbox: {},
  breaker: {},
  refreshAgents: async () => {
    const doc = await hiveRegistry();
    set({ agents: Object.values(doc.agents) });
  },
  select: (id) => set({ selected: id }),
  setInbox: (agentId, msgs) => set((s) => ({ inbox: { ...s.inbox, [agentId]: msgs } })),
  setBreaker: (b) => set((s) => ({ breaker: { ...s.breaker, [b.agent_id]: b } }))
}));
