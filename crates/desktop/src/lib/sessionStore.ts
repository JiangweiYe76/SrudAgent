import { create } from 'zustand';
import type { Session, Turn, TurnEndReason } from './types';
import { t } from './i18n';
import {
  cancelTurn,
  deleteSession as requestDelete,
  initialize,
  listSessions,
  loadSession,
  newSession,
  onNotify,
  type ListedSession,
  RpcFailure,
  sendPrompt,
  SESSION_NOT_FOUND,
  setSessionTitle,
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

// The `_meta.srud` an update carries, if any.
function srudMeta(params: Json): Json {
  return ((params._meta as Json | undefined)?.srud as Json | undefined) ?? {};
}

// The turn an update belongs to, when it says.
function turnIdOf(params: Json): string | null {
  const id = srudMeta(params).turnId;
  return typeof id === 'string' ? id : null;
}

// How a turn ended, when the update says. The wire names are the ones the UI
// already uses for a turn it closed itself, so a replayed turn and a live one are
// labelled the same.
function endReasonOf(params: Json): TurnEndReason | null {
  const reason = srudMeta(params).turnEndReason;
  return reason === 'completed' ||
    reason === 'interrupted' ||
    reason === 'blocked' ||
    reason === 'error'
    ? reason
    : null;
}

// Finds the turn an update belongs to, and opens one when there is none.
//
// The agent's turn id is the authority: every update names the turn it is part
// of, so a client can place an update without having been there when the turn
// opened — which is all it takes for a replay to land in the turns it belongs
// to. Live, `sendTurn` opened the turn before the request went out and the first
// update's id is adopted into it, so both paths end up with the same shape. A
// replay has no such turn, so one is opened with its text still empty for the
// user message to fill.
function locateTurn(turns: Turn[], turnId: string | null): { turns: Turn[]; index: number } | null {
  const named = turnId === null ? -1 : turns.findIndex((tt) => tt.backendTurnId === turnId);
  if (named >= 0) return { turns, index: named };

  // The turn this client opened, before the agent has named it. An update that
  // names no turn at all lands there too, which is where such an update went
  // before turns were named on the wire.
  const unclaimed = unclaimedTurn(turns);
  if (unclaimed >= 0) {
    const next = [...turns];
    next[unclaimed] = { ...next[unclaimed], backendTurnId: turnId ?? undefined };
    return { turns: next, index: unclaimed };
  }

  if (turnId === null) return null;
  const opened: Turn = {
    id: uid('t'),
    userInput: '',
    steps: [],
    backendTurnId: turnId,
    createdAt: Date.now(),
  };
  return { turns: [...turns, opened], index: turns.length };
}

// The newest turn the agent has not named yet, or -1. Backwards because the turn
// a client opened is its newest one, while every older turn was named by the
// update that opened it.
function unclaimedTurn(turns: Turn[]): number {
  for (let i = turns.length - 1; i >= 0; i -= 1) {
    if (turns[i].backendTurnId === undefined) return i;
  }
  return -1;
}

// Applies one `session/update` notification to the session it names.
function applyUpdate(session: Session, params: Json): Session | null {
  const update = params.update as Json | undefined;
  if (!update) return null;
  const kind = String(update.sessionUpdate ?? '');

  // A title change is session-level, not turn-level, so it applies whether or
  // not a turn is open — the first prompt names the session before any output.
  if (kind === 'session_info_update') {
    const title = typeof update.title === 'string' ? update.title : null;
    return { ...session, title, updatedAt: Date.now() };
  }

  // Read before a turn is located, so a turn is never opened for an update that
  // turns out to carry nothing.
  const userText = kind === 'user_message_chunk' ? textOf(update.content) : null;
  if (kind === 'user_message_chunk' && userText === null) return null;

  const found = locateTurn(session.turns, turnIdOf(params));
  if (!found) return null;
  const { index } = found;

  const turn = { ...found.turns[index] };

  if (userText !== null) {
    // A user message is not a step — steps are what the agent did — so it settles
    // the turn's text and nothing else. That text is what fills in a turn which
    // came from the log; a turn this client opened already knows it, because it is
    // what was sent.
    turn.userInput ||= userText;
  } else {
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
      let matched = false;
      for (const step of steps) {
        const tc = step.toolCalls.find((c) => c.id === callId);
        if (tc) {
          tc.result = resultOf(update);
          // The backend states whether the tool failed rather than leaving it to
          // be read off the text, which the UI needs before it can render
          // anything.
          const raw = update.rawOutput as Json | undefined;
          tc.isError =
            raw !== null && typeof raw === 'object' && 'is_error' in raw
              ? Boolean((raw as Json).is_error)
              : undefined;
          matched = true;
          break;
        }
      }
      if (!matched) return null;
    } else {
      return null;
    }

    turn.steps = steps;
  }

  // Copied before the turn is written back, so the session's own array is never
  // mutated: the store compares by reference to decide what to re-render.
  const next = [...found.turns];
  next[index] = turn;
  // A turn is closed by the update that ended it, which for a replayed one is the
  // only place the reason is: there is no `session/prompt` response to carry it.
  const end = endReasonOf(params);
  if (end) next[index] = { ...turn, endReason: end, endedAt: Date.now() };
  return { ...session, turns: next, updatedAt: Date.now() };
}

interface SessionState {
  sessions: Session[];
  activeId: string;
  // Set when the backend handshake or a request fails; shown by the UI.
  initError: string | null;
  // Set while a session's log is being replayed into the store. The conversation
  // is still arriving then, so a prompt sent now would interleave with what is
  // coming back.
  reopening: boolean;
  init: () => Promise<void>;
  select: (id: string) => void;
  addSession: () => void;
  sendTurn: (userInput: string) => void;
  stopTurn: () => void;
  renameSession: (id: string, title: string) => void;
  deleteSession: (id: string) => void;
}

// A session is busy while its newest turn has not been closed. Cancellation is
// cooperative, so the turn stays open until `session/prompt` resolves — the
// stop button must stay enabled for that whole window.
export function isBusy(session: Session | undefined): boolean {
  if (!session || session.turns.length === 0) return false;
  return session.turns[session.turns.length - 1].endReason === undefined;
}

// A session starts as a local draft: no backend session and no workspace
// directory until the first message materializes it. The agent derives a title
// from the first message and announces it over `session/update`.
function freshSession(): Session {
  const now = Date.now();
  return {
    id: uid('s'),
    backendId: null,
    cwd: '',
    loaded: true,
    title: null,
    turns: [],
    createdAt: now,
    updatedAt: now,
  };
}

// A session the agent listed, whose conversation is not on screen yet.
//
// `loaded: false` is what separates it from a session with nothing said in it: the
// two would otherwise look identical, and the sidebar would call a conversation
// that has not arrived an empty one.
function listedSession(info: ListedSession, now: number): Session {
  return {
    id: uid('s'),
    backendId: info.sessionId,
    cwd: info.cwd,
    loaded: false,
    title: info.title,
    turns: [],
    createdAt: now,
    updatedAt: info.updatedAt ?? now,
  };
}

// How many turns a confirmation dialog should mention when deleting: a session
// holding a conversation is worth warning about more loudly than an empty one.
//
// A session whose conversation has not been loaded has none to count, and counts
// as none: the dialog would otherwise have to claim it is empty, which nobody
// knows until it has been opened.
export function deleteWarning(session: Session | undefined): number {
  if (!session?.loaded) return 0;
  return session.turns.length;
}

// The label shown for a session that has no title yet.
export function displayTitle(session: Session | undefined): string {
  return session?.title?.trim() || t('app.newSession');
}

export const useSessionStore = create<SessionState>((set, get) => ({
  sessions: [],
  activeId: '',
  initError: null,
  reopening: false,

  init: async () => {
    try {
      await initialize();
      onNotify((params) => {
        const target = String(params.sessionId ?? '');
        const { sessions } = get();
        let changed = false;
        const next = sessions.map((s) => {
          if (s.backendId !== target) return s;
          const updated = applyUpdate(s, params);
          if (updated) {
            changed = true;
            return updated;
          }
          return s;
        });
        if (changed) set({ sessions: next });
      });
      // The agent is asked what sessions exist before anything is shown, so a
      // restart does not open on an empty app. Its list is the whole history: a
      // session outlives the run that made it, so this is where the ones from
      // earlier runs come from.
      const listed = await listSessions();
      const now = Date.now();
      const sessions = listed.map((info) => listedSession(info, now));
      // A draft so there is always something to type into. It leads the list
      // because it is where a new conversation starts.
      const draft = freshSession();
      set({
        sessions: [draft, ...sessions],
        activeId: draft.id,
        initError: null,
      });
    } catch (err) {
      set({ initError: String(err) });
    }
  },

  select: (id) => {
    set({ activeId: id });
    // A session the agent listed has a conversation that is not on screen yet, so
    // it is asked for on the way to being shown. Already-loaded and draft
    // sessions are left alone: loading one again would replay it on top of itself.
    const session = get().sessions.find((s) => s.id === id);
    if (!session?.backendId || session.loaded) return;
    // Marked before the request rather than after it answers: a second click while
    // the first load is still in flight must not ask for the same conversation
    // again, and waiting for the reply would leave that window open.
    useSessionStore.setState((s) => ({
      sessions: s.sessions.map((sess) => (sess.id === id ? { ...sess, loaded: true } : sess)),
      reopening: true,
    }));
    void loadSession(session.backendId, session.cwd)
      .catch((err: unknown) => {
        // A session the agent will not name is one this app cannot show, and
        // showing it as an empty conversation would be a lie. It goes, and the
        // sidebar is one row shorter rather than one wrong.
        const gone = err instanceof RpcFailure && err.code === SESSION_NOT_FOUND;
        if (!gone) {
          useSessionStore.setState({ initError: String(err) });
          return;
        }
        removeSession(id);
      })
      .finally(() => {
        useSessionStore.setState({ reopening: false });
      });
  },

  addSession: () => {
    const draft = freshSession();
    set((s) => ({ sessions: [draft, ...s.sessions], activeId: draft.id }));
  },

  sendTurn: (userInput) => {
    const state = get();
    const session = state.sessions.find((s) => s.id === state.activeId);
    // A turn refused mid-replay would be written into a conversation that is only
    // half back, and the two would end up interleaved.
    if (!session || state.reopening || isBusy(session)) return;
    const sessionId = session.id;
    const now = Date.now();
    const turn: Turn = { id: uid('t'), userInput, steps: [], createdAt: now };

    set((s) => ({
      sessions: s.sessions.map((sess) =>
        sess.id === sessionId ? { ...sess, turns: [...sess.turns, turn], updatedAt: now } : sess,
      ),
    }));

    void (async () => {
      let backendId = get().sessions.find((s) => s.id === sessionId)?.backendId ?? null;
      if (!backendId) {
        try {
          // No working directory chosen yet: the app has no directory picker, so the
          // agent gives the session a workspace of its own. That is also what it
          // did when this passed the launch directory, because the agent only
          // uses a directory that exists — so nothing changes until a picker
          // exists and a user picks somewhere.
          const created = await newSession('');
          const draftTitle = get().sessions.find((s) => s.id === sessionId)?.title?.trim() || '';
          useSessionStore.setState((s) => ({
            sessions: s.sessions.map((sess) =>
              sess.id === sessionId ? { ...sess, backendId: created } : sess,
            ),
          }));
          backendId = created;
          if (draftTitle) {
            try {
              await setSessionTitle(backendId, draftTitle);
            } catch (err) {
              useSessionStore.setState({ initError: String(err) });
            }
          }
        } catch (err) {
          closeTurn(sessionId, turn.id, 'error', String(err));
          return;
        }
      }

      try {
        const reason = await sendPrompt(backendId, userInput);
        closeTurn(sessionId, turn.id, mapStopReason(reason));
      } catch (err) {
        closeTurn(sessionId, turn.id, 'error', String(err));
      }
    })();
  },

  stopTurn: () => {
    const session = get().sessions.find((s) => s.id === get().activeId);
    if (!isBusy(session) || !session?.backendId) return;
    // The turn is not closed here: `session/cancel` only signals the token, and
    // the open `session/prompt` is what resolves it, as `interrupted`.
    void cancelTurn(session.backendId).catch((err) => set({ initError: String(err) }));
  },

  renameSession: (id, title) => {
    const session = get().sessions.find((s) => s.id === id);
    if (!session) return;
    if (!session.backendId) {
      // A draft has no agent to echo a rename, so the title is stored locally.
      // It is pushed to the backend when the draft materializes.
      const next = title.trim() || null;
      set((s) => ({
        sessions: s.sessions.map((sess) => (sess.id === id ? { ...sess, title: next } : sess)),
      }));
      return;
    }
    // The agent is the source of truth and echoes the accepted title back as a
    // `session_info_update`, so nothing is written locally here. A blank title
    // clears the name rather than storing an empty one.
    void setSessionTitle(session.backendId, title.trim()).catch((err) =>
      set({ initError: String(err) }),
    );
  },

  deleteSession: (id) => {
    const session = get().sessions.find((s) => s.id === id);
    if (!session) return;
    if (!session.backendId) {
      removeSession(id);
      return;
    }
    void requestDelete(session.backendId)
      .then(() => removeSession(id))
      .catch((err) => set({ initError: String(err) }));
  },
}));

// Drops a session from the store once the agent has confirmed the delete (or
// immediately for a draft, which holds nothing backend-side), and picks what
// to show next.
//
// The row is removed only after the agent agrees, so a failed delete leaves the
// session visible instead of silently losing it from the UI. A turn still in
// flight keeps streaming updates into the old turn, which `applyUpdate` no
// longer matches — harmless, since the session is gone from the list.
function removeSession(id: string) {
  useSessionStore.setState((s) => {
    const remaining = s.sessions.filter((sess) => sess.id !== id);
    if (remaining.length === s.sessions.length) return s;
    // Deleting the session on screen moves the view to its neighbour; deleting
    // the last one leaves no session to fall back to.
    const activeId = s.activeId === id ? (remaining[0]?.id ?? '') : s.activeId;
    return { sessions: remaining, activeId };
  });
  if (!useSessionStore.getState().activeId) useSessionStore.getState().addSession();
}

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
