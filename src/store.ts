import { create } from 'zustand';
import type { Message, RegistryEntry } from './api';
import { hiveRegistry } from './api';

interface AppState {
  agents: RegistryEntry[];
  selected: string | null;
  inbox: Record<string, Message[]>;
  refreshAgents: () => Promise<void>;
  select: (id: string | null) => void;
  setInbox: (agentId: string, msgs: Message[]) => void;
}

export const useApp = create<AppState>((set) => ({
  agents: [],
  selected: null,
  inbox: {},
  refreshAgents: async () => {
    const agents = await hiveRegistry();
    set({ agents });
  },
  select: (id) => set({ selected: id }),
  setInbox: (agentId, msgs) => set((s) => ({ inbox: { ...s.inbox, [agentId]: msgs } }))
}));
