import { create } from 'zustand';
import type { Session, Turn, TurnEndReason } from './types';
import { t } from './i18n';
import {
  cancelTurn,
  defaultCwd,
  initialize,
  newSession,
  onNotify,
  sendPrompt,
  type Json,
} from './acp';

let idCounter = 1000;
const uid = (prefix: string) => `${prefix}${idCounter++}`;

// ACP stop reasons mapped onto the UI's turn-end vocabulary.
function mapStopReason(reason: string): TurnEndReason {
  return reason === 'cancelled' ? 'interrupted' : 'completed';
}

function textOf(block: unknown): string | null {
  const b = block as Json | undefined;
  if (b && b.type === 'text' && typeof b.text === 'string') return b.text;
  return null;
}

// The result text of a tool_call_update: content blocks first, raw output
// as a fallback.
function resultOf(update: Json): string {
  const items = update.content as Json[] | undefined;
  if (Array.isArray(items)) {
    const text = items
      .map((item) => textOf(item) ?? textOf(item?.content))
      .filter((s): s is string => s !== null)
      .join('\n');
    if (text) return text;
  }
  return update.rawOutput === undefined ? '' : JSON.stringify(update.rawOutput);
}

// Applies one `session/update` notification to a session's newest turn.
// The UI turn is opened by `sendTurn` before the request goes out, so the
// newest turn is always the one the backend is currently writing.
function applyUpdate(session: Session, params: Json): Session | null {
  const update = params.update as Json | undefined;
  if (!update || session.turns.length === 0) return null;
  const kind = String(update.sessionUpdate ?? '');
  if (kind === 'user_message_chunk') return null;

  const turns = [...session.turns];
  let turn = { ...turns[turns.length - 1] };
  const steps = turn.steps.map((st) => ({ ...st, toolCalls: [...st.toolCalls] }));
  const last = steps[steps.length - 1];

  if (kind === 'agent_thought_chunk') {
    const text = textOf(update.content);
    if (text === null) return null;
    // Reasoning opens a step the way message text does — a step is one
    // sampling, and thinking is what that sampling produced first. Reasoning
    // seen after the step's tool calls belongs to the next sampling.
    if (!last || last.toolCalls.length > 0) {
      steps.push({ id: uid('st'), thought: text, assistantText: '', toolCalls: [] });
    } else {
      last.thought = (last.thought ?? '') + text;
    }
  } else if (kind === 'agent_message_chunk') {
    const text = textOf(update.content);
    if (text === null) return null;
    // A step ends at its tool calls; text after them opens a new step.
    if (!last || last.toolCalls.length > 0) {
      steps.push({ id: uid('st'), assistantText: text, toolCalls: [] });
    } else {
      last.assistantText += text;
    }
  } else if (kind === 'tool_call') {
    const call = {
      id: String(update.toolCallId ?? uid('tc')),
      name: String(update.title ?? 'tool'),
      args: update.rawInput === undefined ? '' : JSON.stringify(update.rawInput),
    };
    // The call belongs to the current step — one sampling streams its text
    // then issues its calls. Only a fresh turn (no step yet) opens a new one.
    if (last) {
      last.toolCalls.push(call);
    } else {
      steps.push({ id: uid('st'), assistantText: '', toolCalls: [call] });
    }
  } else if (kind === 'tool_call_update') {
    const callId = String(update.toolCallId ?? '');
    let found = false;
    for (const step of steps) {
      const tc = step.toolCalls.find((c) => c.id === callId);
      if (tc) {
        tc.result = resultOf(update);
        found = true;
        break;
      }
    }
    if (!found) return null;
  } else {
    return null;
  }

  turn = { ...turn, steps };
  turns[turns.length - 1] = turn;
  return { ...session, turns, updatedAt: Date.now() };
}

interface SessionState {
  sessions: Session[];
  activeId: string;
  // Set when the backend handshake or a request fails; shown by the UI.
  initError: string | null;
  init: () => Promise<void>;
  select: (id: string) => void;
  addSession: () => void;
  sendTurn: (userInput: string) => void;
  stopTurn: () => void;
}

// A session is busy while its newest turn has not been closed. Cancellation is
// cooperative, so the turn stays open until `session/prompt` resolves — the
// stop button must stay enabled for that whole window.
export function isBusy(session: Session | undefined): boolean {
  if (!session || session.turns.length === 0) return false;
  return session.turns[session.turns.length - 1].endReason === undefined;
}

function freshSession(id: string): Session {
  const now = Date.now();
  return { id, title: t('app.newSession'), turns: [], createdAt: now, updatedAt: now };
}

export const useSessionStore = create<SessionState>((set, get) => ({
  sessions: [],
  activeId: '',
  initError: null,

  init: async () => {
    try {
      await initialize();
      const sessionId = await newSession(await defaultCwd());
      onNotify((params) => {
        const target = String(params.sessionId ?? '');
        const { sessions } = get();
        let changed = false;
        const next = sessions.map((s) => {
          if (s.id !== target) return s;
          const updated = applyUpdate(s, params);
          if (updated) {
            changed = true;
            return updated;
          }
          return s;
        });
        if (changed) set({ sessions: next });
      });
      set({ sessions: [freshSession(sessionId)], activeId: sessionId, initError: null });
    } catch (err) {
      set({ initError: String(err) });
    }
  },

  select: (id) => set({ activeId: id }),

  addSession: () => {
    void defaultCwd()
      .then((cwd) => newSession(cwd))
      .then((sessionId) => {
        set((s) => ({ sessions: [freshSession(sessionId), ...s.sessions], activeId: sessionId }));
      })
      .catch((err) => set({ initError: String(err) }));
  },

  sendTurn: (userInput) => {
    const state = get();
    const now = Date.now();
    const turn: Turn = { id: uid('t'), userInput, steps: [], createdAt: now };

    set((s) => ({
      sessions: s.sessions.map((sess) =>
        sess.id === s.activeId ? { ...sess, turns: [...sess.turns, turn], updatedAt: now } : sess,
      ),
    }));

    void sendPrompt(state.activeId, userInput)
      .then((reason) => {
        closeTurn(state.activeId, turn.id, mapStopReason(reason));
      })
      .catch((err) => {
        closeTurn(state.activeId, turn.id, 'error', String(err));
      });
  },

  stopTurn: () => {
    const sessionId = get().activeId;
    if (!isBusy(get().sessions.find((s) => s.id === sessionId))) return;
    // The turn is not closed here: `session/cancel` only signals the token, and
    // the open `session/prompt` is what resolves it, as `interrupted`.
    void cancelTurn(sessionId).catch((err) => set({ initError: String(err) }));
  },
}));

function closeTurn(sessionId: string, turnId: string, endReason: TurnEndReason, error?: string) {
  useSessionStore.setState((s) => ({
    sessions: s.sessions.map((sess) => {
      if (sess.id !== sessionId) return sess;
      return {
        ...sess,
        turns: sess.turns.map((tt) =>
          tt.id === turnId ? { ...tt, endReason, endedAt: Date.now() } : tt,
        ),
        updatedAt: Date.now(),
      };
    }),
    ...(error ? { initError: error } : {}),
  }));
}
