import { create } from 'zustand';
import type { Session, Turn } from './types';
import { seedSessions } from '@/data/mockData';
import { t } from './i18n';

let idCounter = 1000;
const uid = (prefix: string) => `${prefix}${idCounter++}`;

interface SessionState {
  sessions: Session[];
  activeId: string;
  select: (id: string) => void;
  addSession: () => void;
  sendTurn: (userInput: string) => void;
}

export const useSessionStore = create<SessionState>((set) => ({
  sessions: seedSessions,
  activeId: seedSessions[0]?.id ?? '',

  select: (id) => set({ activeId: id }),

  addSession: () =>
    set((state) => {
      const now = Date.now();
      const session: Session = {
        id: uid('s'),
        title: t('app.newSession'),
        turns: [],
        createdAt: now,
        updatedAt: now,
      };
      return { sessions: [session, ...state.sessions], activeId: session.id };
    }),

  // Mock a turn with a single completed step until the backend is wired up.
  sendTurn: (userInput) =>
    set((state) => ({
      sessions: state.sessions.map((s) => {
        if (s.id !== state.activeId) return s;
        const now = Date.now();
        const turn: Turn = {
          id: uid('t'),
          userInput,
          steps: [{ id: uid('st'), assistantText: t('mock.reply'), toolCalls: [] }],
          endReason: 'completed',
          createdAt: now,
          endedAt: now + 1200,
        };
        return { ...s, turns: [...s.turns, turn], updatedAt: now };
      }),
    })),
}));
