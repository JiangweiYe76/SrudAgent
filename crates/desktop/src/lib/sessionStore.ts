import { create } from 'zustand';
import { toast } from 'sonner';
import type {
  Attachment,
  ModelOptions,
  Session,
  Step,
  ToolCall,
  Turn,
  TurnEndReason,
  UsageState,
} from './types';
import { t } from './i18n';
import { loadDrafts, saveDrafts } from './drafts';
import { draftModelOptions, useProviderStore } from './providers';
import {
  cancelTurn,
  deleteSession as requestDelete,
  initialize,
  listSessions,
  loadSession,
  newSession,
  onNotify,
  parseModelOptions,
  setSessionModel,
  type ListedSession,
  type LoadedSession,
  type LoadedTurnEnd,
  RpcFailure,
  sendPrompt,
  steerTurn as requestSteer,
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

function imageOf(block: unknown): Attachment | null {
  const b = block as Json | undefined;
  if (b && b.type === 'image' && typeof b.data === 'string' && typeof b.mimeType === 'string') {
    return { id: uid('img'), mime: b.mimeType, data: b.data };
  }
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

// Whether a tool call is still being written out by the model.
//
// ACP's `pending` is documented as a call whose input is streaming, and it is the
// default, so an agent that leaves it unset is saying the same thing by saying
// nothing — which is what SrudAgent's own announcement does.
//
// An unset status is read as pending only when there is no input either. A call
// that arrived whole without a status has plainly stopped waiting for its
// arguments, and treating it as pending would put the waiting mark over a
// complete one.
function isPending(update: Json): boolean {
  const status = update.status;
  if (status === 'pending') return true;
  if (status !== undefined) return false;
  return update.rawInput === undefined;
}

// The parts of a call a `tool_call` update describes.
//
// A call arrives as an announcement with a name and no input, and the input comes
// later — either as an update or, from a replayed log, in the same update. Which
// is why `args` is empty rather than absent for one that has none yet.
function toolCallFields(update: Json, id: string): ToolCall {
  return {
    id,
    name: String(update.name ?? update.title ?? 'tool'),
    args: update.rawInput === undefined ? '' : JSON.stringify(update.rawInput),
    pending: isPending(update),
  };
}

// The call with this id, wherever it is among the steps.
function findCall(steps: Step[], id: string): ToolCall | undefined {
  for (const step of steps) {
    const call = step.toolCalls.find((c) => c.id === id);
    if (call) return call;
  }
  return undefined;
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
  // names no turn at all lands there too.
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
//
// `replaying` marks the updates a `session/load` hands over: they carry a
// conversation from the past, so they must not restamp the session as though
// it had just been active.
function applyUpdate(session: Session, params: Json, replaying: boolean): Session | null {
  const update = params.update as Json | undefined;
  if (!update) return null;
  const kind = String(update.sessionUpdate ?? '');

  // A title change is session-level, not turn-level, so it applies whether or
  // not a turn is open — the first prompt names the session before any output.
  if (kind === 'session_info_update') {
    const title = typeof update.title === 'string' ? update.title : null;
    return { ...session, title, updatedAt: replaying ? session.updatedAt : Date.now() };
  }

  // A usage report is session-level too: it describes the context the session
  // is working in, not one turn's output, and it applies whether or not a turn
  // is open. The stable fields give the newest occupancy; the `_meta.srud.usage`
  // breakdown adds up across reports for the cache hit rate.
  if (kind === 'usage_update') {
    const used = Number(update.used);
    const size = Number(update.size);
    if (!Number.isFinite(used) || !Number.isFinite(size)) return null;
    const breakdown = srudMeta(params).usage as Json | undefined;
    // A count the breakdown leaves out reads as zero: providers report
    // differently, and a missing field means "not reported", not "unknown".
    const step = (field: string) =>
      typeof breakdown?.[field] === 'number' ? breakdown[field] : 0;
    const total = (field: keyof UsageState) => (session.usage?.[field] ?? 0) + step(field);
    return {
      ...session,
      usage: {
        used,
        size,
        inputTokens: total('inputTokens'),
        outputTokens: total('outputTokens'),
        cachedInputTokens: total('cachedInputTokens'),
        cacheWriteInputTokens: total('cacheWriteInputTokens'),
        reasoningOutputTokens: total('reasoningOutputTokens'),
      },
      updatedAt: replaying ? session.updatedAt : Date.now(),
    };
  }

  // So is a model change: it was already recorded when this arrived, since the
  // notification is how another client's switch reaches this one. The whole
  // selector comes with it, so the list and the selection cannot disagree.
  if (kind === 'config_option_update') {
    const model = parseModelOptions(update.configOptions);
    if (!model) return null;
    return { ...session, model, updatedAt: replaying ? session.updatedAt : Date.now() };
  }

  // Read before a turn is located, so a turn is never opened for an update that
  // turns out to carry nothing.
  const userText = kind === 'user_message_chunk' ? textOf(update.content) : null;
  const userImage = kind === 'user_message_chunk' ? imageOf(update.content) : null;
  if (kind === 'user_message_chunk' && userText === null && userImage === null) return null;

  const found = locateTurn(session.turns, turnIdOf(params));
  if (!found) return null;
  const { index } = found;

  const turn = { ...found.turns[index] };

  if (kind === 'user_message_chunk') {
    // An image is recorded after the text it was sent with, so it extends the
    // turn instead of opening anything of its own.
    if (userImage !== null) {
      turn.images = [...(turn.images ?? []), userImage];
    }
    if (userText !== null) {
      // A user message is not a step — steps are what the agent did — so it
      // settles the turn's text and nothing else. That text is what fills in a
      // turn which came from the log; a turn this client opened already knows
      // it, because it is what was sent.
      // A steered message is a new user input arriving mid-turn. Text matching
      // what the turn already holds is the sent message coming back, so it
      // changes nothing; anything else is a steer and becomes its own bubble.
      if (!turn.userInput) {
        turn.userInput = userText;
      } else if (userText !== turn.userInput) {
        turn.steeredInputs = [...(turn.steeredInputs ?? []), userText];
      }
    }
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
      const id = String(update.toolCallId ?? uid('tc'));
      const existing = findCall(steps, id);
      // A call that is already on screen is being described further, not added:
      // the agent announces a tool before the model has written its arguments,
      // and an agent that sends the whole call twice must not produce two rows.
      if (existing) {
        Object.assign(existing, toolCallFields(update, id));
        return null;
      }
      const call = toolCallFields(update, id);
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
          // An update that arrives at all means the agent has moved past naming
          // the call, whether or not it says where it moved to.
          if (!isPending(update)) tc.pending = false;
          if (update.rawInput !== undefined) tc.args = JSON.stringify(update.rawInput);
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
  return { ...session, turns: next, updatedAt: replaying ? session.updatedAt : Date.now() };
}

// ---------------------------------------------------------------------------
// Batching of session/update notifications (production only)
// ---------------------------------------------------------------------------
//
// A history replay can fire hundreds of `session/update` notifications in a few
// milliseconds. Applying each one as a separate `set()` re-renders the transcript
// hundreds of times. These accumulate notifications and flush them in one state
// write per animation frame. Only used outside tests (see onNotify).

const pendingUpdates: Json[] = [];
let flushScheduled = false;

function flushPendingUpdates() {
  flushScheduled = false;
  if (pendingUpdates.length === 0) return;

  const batch = pendingUpdates.splice(0);
  const byTarget = new Map<string, Json[]>();
  for (const params of batch) {
    const target = String(params.sessionId ?? '');
    let list = byTarget.get(target);
    if (!list) {
      list = [];
      byTarget.set(target, list);
    }
    list.push(params);
  }

  const state = useSessionStore.getState();
  const sessions = state.sessions;
  let next = sessions;
  let changed = false;

  for (const [target, updates] of byTarget) {
    const index = next.findIndex((s) => s.backendId === target);
    if (index < 0) continue;

    let session = next[index];
    for (const params of updates) {
      const updated = applyUpdate(session, params, replaying.has(target));
      if (updated) {
        session = updated;
        changed = true;
      }
    }

    if (session !== next[index]) {
      if (next === sessions) next = [...sessions];
      next[index] = session;
    }
  }

  if (changed) useSessionStore.setState({ sessions: next });
}

// Flush any pending batched updates synchronously. Called from select's
// .finally() to ensure the conversation is fully applied before the loading
// indicator goes away.
export function flushPendingReplayUpdates() {
  flushPendingUpdates();
}

interface SessionState {
  sessions: Session[];
  activeId: string;
  // Set when the backend handshake or a request fails; shown by the UI.
  initError: string | null;
  // Loads in flight. Counted rather than flagged: a session that cannot be loaded
  // hands the view to its neighbour, so a second load can start before the first
  // has finished failing, and one flag cleared by the first to settle would open
  // the prompt while the second is still arriving.
  reopeningCount: number;
  // Sessions a delete is in flight for. The row stays until the agent agrees —
  // a refused delete must lose nothing — and this is what the sidebar shows
  // while it waits.
  deletingIds: string[];
  // What each session's input box holds, keyed by `draftKeyOf`.
  drafts: Record<string, string>;
  setDraft: (id: string, text: string) => void;
  // Images pasted into each session's box, keyed like drafts. Memory only: an
  // image is not kept across restarts, unlike the draft it was attached to.
  attachments: Record<string, Attachment[]>;
  setAttachments: (id: string, attachments: Attachment[]) => void;
  init: () => Promise<void>;
  select: (id: string) => void;
  addSession: () => void;
  // `sessionId` names the conversation to run in; the visible box leaves it
  // out and the active session is used. Images are sent with the text and
  // cannot be queued: a turn already running takes text only.
  sendTurn: (userInput: string, sessionId?: string, attachments?: Attachment[]) => void;
  queueFollowup: (text: string) => void;
  removeFollowup: (index: number) => void;
  steerFollowup: (index: number) => void;
  stopTurn: () => void;
  setModel: (model: string) => void;
  renameSession: (id: string, title: string) => void;
  deleteSession: (id: string) => void;
}

// A session's conversation is on its way in.
export function isReopening(state: Pick<SessionState, 'reopeningCount'>): boolean {
  return state.reopeningCount > 0;
}

// What a session's input box shows: the draft filed under its key, or empty
// when it has none.
export function draftFor(state: Pick<SessionState, 'sessions' | 'drafts'>, id: string): string {
  return state.drafts[draftKeyOf(state.sessions, id)] ?? '';
}

// A stable empty list, so a box holding no images keeps returning the same
// reference and a selector on it does not re-render on every store write.
const NO_ATTACHMENTS: Attachment[] = [];

// The images a session's input box holds.
export function attachmentsFor(
  state: Pick<SessionState, 'sessions' | 'attachments'>,
  id: string,
): Attachment[] {
  return state.attachments[draftKeyOf(state.sessions, id)] ?? NO_ATTACHMENTS;
}

// The backend sessions whose `session/load` is still in flight. The updates they
// hand over replay a conversation from the past, so they must not restamp the
// session's `updatedAt` and make a stale row read as "just now".
const replaying = new Set<string>();

function beginReopen(backendId: string) {
  replaying.add(backendId);
  useSessionStore.setState((s) => ({ reopeningCount: s.reopeningCount + 1 }));
}

function endReopen(backendId: string) {
  replaying.delete(backendId);
  // Floored, so a stray end cannot leave the prompt locked with nothing in flight.
  useSessionStore.setState((s) => ({ reopeningCount: Math.max(0, s.reopeningCount - 1) }));
}

// A session is busy while its newest turn has not been closed. Cancellation is
// cooperative, so the turn stays open until `session/prompt` resolves — the
// stop button must stay enabled for that whole window.
export function isBusy(session: Session | undefined): boolean {
  if (!session || session.turns.length === 0) return false;
  return session.turns[session.turns.length - 1].endReason === undefined;
}

// The key a session's draft is filed under: the backend id once the session
// has one, its local id until then.
function draftKeyOf(sessions: Session[], id: string): string {
  return sessions.find((s) => s.id === id)?.backendId ?? id;
}

// The stored drafts whose session the agent still lists.
function listedDrafts(sessions: Session[]): Record<string, string> {
  const live = new Set(
    sessions.map((s) => s.backendId).filter((id): id is string => id !== null),
  );
  const drafts: Record<string, string> = {};
  for (const [key, text] of Object.entries(loadDrafts())) {
    if (live.has(key)) drafts[key] = text;
  }
  return drafts;
}

function withoutDraft(drafts: Record<string, string>, key: string): Record<string, string> {
  if (!(key in drafts)) return drafts;
  const next = { ...drafts };
  delete next[key];
  return next;
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
    model: null,
    turns: [],
    followups: [],
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
    // Known once the conversation is loaded: the model comes back with it.
    model: null,
    turns: [],
    followups: [],
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

// The app's one handshake and listing, shared by however many callers ask for it.
// React's strict mode mounts the effect that asks twice in development, and a
// second start would ask the agent for the same conversation all over again.
let starting: Promise<void> | null = null;

export const useSessionStore = create<SessionState>((set, get) => ({
  sessions: [],
  activeId: '',
  initError: null,
  reopeningCount: 0,
  deletingIds: [],
  drafts: {},
  attachments: {},

  init: () => {
    starting ??= start();
    return starting;

    async function start() {
      try {
        await initialize();
        onNotify((params) => {
          const target = String(params.sessionId ?? '');
          const isTest = typeof process !== 'undefined' && (process.env?.VITEST || process.env?.NODE_ENV === 'test');

          if (isTest) {
            // Tests expect synchronous application: fire() is called and the
            // state is checked immediately. Batching would defer the update.
            const { sessions } = get();
            let changed = false;
            const next = sessions.map((s) => {
              if (s.backendId !== target) return s;
              const updated = applyUpdate(s, params, replaying.has(target));
              if (updated) {
                changed = true;
                return updated;
              }
              return s;
            });
            if (changed) set({ sessions: next });
          } else {
            // Production: batch updates to reduce React re-renders during replay.
            pendingUpdates.push(params);
            if (!flushScheduled) {
              flushScheduled = true;
              if (typeof requestAnimationFrame !== 'undefined') {
                requestAnimationFrame(flushPendingUpdates);
              } else {
                setTimeout(flushPendingUpdates, 0);
              }
            }
          }
        });
        // The agent is asked what sessions exist before anything is shown, so a
        // restart lands on the conversation the user left off in rather than on an
        // empty app. Its list is the whole history: a session outlives the run that
        // made it, so this is where the ones from earlier runs come from. It comes
        // live sessions first and stored ones newest first, so the first is the one
        // to reopen.
        const listed = await listSessions();
        const now = Date.now();
        const sessions = listed.map((info) => listedSession(info, now));
        const drafts = listedDrafts(sessions);
        // Rewritten with the live drafts, so keys for sessions that are gone do
        // not sit in storage.
        saveDrafts(drafts);
        if (sessions.length > 0) {
          set({ sessions, drafts, initError: null });
          get().select(sessions[0].id);
          return;
        }
        // Nothing to go back to, so a draft to type into: with no session at all
        // there is nowhere to put what the user is about to say.
        const draft = freshSession();
        set({ sessions: [draft], activeId: draft.id, initError: null });
      } catch (err) {
        set({ initError: String(err) });
        // A start that failed is not remembered, so asking again is an attempt
        // rather than a replay of the refusal.
        starting = null;
      }
    }
  },

  setDraft: (id, text) => {
    const key = draftKeyOf(get().sessions, id);
    const drafts = { ...get().drafts };
    // An empty box leaves no entry behind.
    if (text) drafts[key] = text;
    else delete drafts[key];
    set({ drafts });
    saveDrafts(drafts);
  },

  setAttachments: (id, attachments) => {
    const key = draftKeyOf(get().sessions, id);
    const next = { ...get().attachments };
    // An empty box leaves no entry behind.
    if (attachments.length > 0) next[key] = attachments;
    else delete next[key];
    set({ attachments: next });
  },

  select: (id) => {
    set({ activeId: id });
    // A session the agent listed has a conversation that is not on screen yet, so
    // it is asked for on the way to being shown. Already-loaded and draft
    // sessions are left alone: loading one again would replay it on top of itself.
    const session = get().sessions.find((s) => s.id === id);
    if (!session?.backendId || session.loaded) return;
    const backendId = session.backendId;
    // Marked before the request rather than after it answers: a second click while
    // the first load is still in flight must not ask for the same conversation
    // again, and waiting for the reply would leave that window open.
    useSessionStore.setState((s) => ({
      sessions: s.sessions.map((sess) => (sess.id === id ? { ...sess, loaded: true } : sess)),
    }));
    beginReopen(backendId);
    // How each replayed turn ended arrives on the response; the updates carry no
    // turn boundary. Held here until the replay is on screen.
    let loaded: LoadedSession | null = null;
    void loadSession(backendId, session.cwd)
      .then((result) => {
        loaded = result;
        // The reply says which model the replayed conversation was left on.
        // Written before the updates stop arriving, so the picker is right from
        // the moment the conversation appears.
        if (result.model) setModelOf(id, result.model);
      })
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
        flushPendingReplayUpdates();
        // After the flush: a turn exists in the store only once the update that
        // opened it has been applied, and the replay is applied in batches.
        if (loaded) closeReplayedTurns(id, loaded.turns);
        endReopen(backendId);
        // A turn that closed while the replay was in flight had its follow-up
        // refused; it is owed one now that the conversation is back.
        flushFollowups(id);
      });
  },

  addSession: () => {
    // The session on screen already reads as "Start a new chat", so another
    // draft would only stack an identical empty row in the sidebar. `loaded`
    // guards the other way: a listed conversation not yet on screen has no
    // turns simply because it has not arrived, and that is not an empty chat.
    const state = get();
    const active = state.sessions.find((s) => s.id === state.activeId);
    if (active && active.loaded && active.turns.length === 0) return;
    const draft = freshSession();
    set((s) => ({ sessions: [draft, ...s.sessions], activeId: draft.id }));
  },

  sendTurn: (userInput, target, images) => {
    const state = get();
    const session = state.sessions.find((s) => s.id === (target ?? state.activeId));
    // A turn refused mid-replay would be written into a conversation that is only
    // half back, and the two would end up interleaved.
    if (!session || state.reopeningCount > 0) return;
    const attachments = images?.length ? images : undefined;
    // While a turn is running, a submitted message is queued as a follow-up
    // rather than starting a second turn. A drain that finds the turn still busy
    // simply leaves its message waiting instead of queueing it twice.
    if (isBusy(session)) {
      // A running turn takes text only, so an image is never queued behind one.
      if (!target && !attachments) get().queueFollowup(userInput);
      return;
    }
    const sessionId = session.id;
    const now = Date.now();
    const turn: Turn = {
      id: uid('t'),
      userInput,
      images: attachments ?? [],
      steps: [],
      createdAt: now,
    };

    // The draft and its images go with the turn they became. Cleared only once
    // the turn is accepted: a refused send leaves them in the box.
    const draftKey = draftKeyOf(state.sessions, sessionId);
    set((s) => {
      const attachments = { ...s.attachments };
      delete attachments[draftKey];
      return {
        sessions: s.sessions.map((sess) =>
          sess.id === sessionId ? { ...sess, turns: [...sess.turns, turn], updatedAt: now } : sess,
        ),
        drafts: withoutDraft(s.drafts, draftKey),
        attachments,
      };
    });
    saveDrafts(get().drafts);

    void (async () => {
      let backendId = get().sessions.find((s) => s.id === sessionId)?.backendId ?? null;
      if (!backendId) {
        try {
          // No working directory chosen yet: the app has no directory picker,
          // so the session is created with an empty one and the agent gives it
          // a workspace of its own under the configuration directory.
          const created = await newSession('');
          // A draft's model choice takes effect on the session it becomes. A
          // switch needs a session id, and this is the first moment there is
          // one: answering the first prompt on the default instead would make
          // the choice look like it had not been made.
          const chosen = get().sessions.find((s) => s.id === sessionId)?.model?.currentValue ?? '';
          let model = created.model;
          if (model && chosen && chosen !== model.currentValue && offers(model, chosen)) {
            try {
              model = (await setSessionModel(created.sessionId, chosen)) ?? model;
            } catch (err) {
              // The session runs on what it was created with; the picker shows
              // the agent's answer rather than the choice that did not take.
              useSessionStore.setState({ initError: String(err) });
            }
          }
          const draftTitle = get().sessions.find((s) => s.id === sessionId)?.title?.trim() || '';
          useSessionStore.setState((s) => {
            // The draft moves to the backend id. Text typed while the backend
            // session was being created is still under the local id here.
            const key = draftKeyOf(s.sessions, sessionId);
            const drafts = { ...s.drafts };
            const text = drafts[key];
            delete drafts[key];
            if (text !== undefined) drafts[created.sessionId] = text;
            const attachments = { ...s.attachments };
            const images = attachments[key];
            delete attachments[key];
            if (images !== undefined) attachments[created.sessionId] = images;
            return {
              sessions: s.sessions.map((sess) =>
                sess.id === sessionId
                  ? { ...sess, backendId: created.sessionId, model }
                  : sess,
              ),
              drafts,
              attachments,
            };
          });
          saveDrafts(useSessionStore.getState().drafts);
          backendId = created.sessionId;
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
        const reason = await sendPrompt(backendId, userInput, attachments ?? []);
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

  queueFollowup: (text) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    const session = get().sessions.find((s) => s.id === get().activeId);
    if (!session) return;
    set((s) => ({
      sessions: s.sessions.map((sess) =>
        sess.id === session.id ? { ...sess, followups: [...(sess.followups ?? []), trimmed] } : sess,
      ),
    }));
    // Clear the input box once queued.
    const key = draftKeyOf(get().sessions, session.id);
    set((s) => ({ drafts: withoutDraft(s.drafts, key) }));
    saveDrafts(get().drafts);
  },

  removeFollowup: (index) => {
    const session = get().sessions.find((s) => s.id === get().activeId);
    if (!session) return;
    set((s) => ({
      sessions: s.sessions.map((sess) =>
        sess.id === session.id
          ? { ...sess, followups: (sess.followups ?? []).filter((_, i) => i !== index) }
          : sess,
      ),
    }));
  },

  steerFollowup: (index) => {
    const session = get().sessions.find((s) => s.id === get().activeId);
    if (!session?.backendId) return;
    const text = (session.followups ?? [])[index];
    if (!text) return;
    // The row leaves the waiting area only once the steer is accepted.
    const drop = () =>
      set((s) => ({
        sessions: s.sessions.map((sess) =>
          sess.id === session.id
            ? { ...sess, followups: (sess.followups ?? []).filter((_, i) => i !== index) }
            : sess,
        ),
      }));
    requestSteer(session.backendId, text)
      .then(drop)
      .catch((err: unknown) => {
        // A refused steer (the turn ended or sealed in between) is retried as
        // the next turn when there is no turn left to steer into; while a turn
        // still runs the message keeps waiting instead of being lost.
        const current = get().sessions.find((s) => s.id === session.id);
        if (current && !isBusy(current)) {
          drop();
          get().sendTurn(text, session.id);
          return;
        }
        set({ initError: String(err) });
      });
  },

  setModel: (model) => {
    const session = get().sessions.find((s) => s.id === get().activeId);
    if (!session) return;
    // A draft has no agent session to switch, so the choice is recorded against
    // the draft and applied when the first prompt creates one. The selector is
    // the same catalog one the header rendered, so the trigger reads the new
    // choice straight away.
    if (!session.backendId) {
      const { providers } = useProviderStore.getState();
      const options = draftModelOptions(providers, model);
      // The catalog no longer offers that model: the draft keeps what it had.
      if (!options || options.currentValue !== model) return;
      set((s) => ({
        sessions: s.sessions.map((sess) =>
          sess.id === session.id ? { ...sess, model: options } : sess,
        ),
      }));
      return;
    }
    // A session with no selector — a host that configured no provider — has
    // nothing to switch between.
    if (!session.model) return;
    const localId = session.id;
    // The agent echoes the accepted selection back over `session/update`, but
    // the reply already carries it: the picker settles from the reply rather
    // than waiting on the broadcast echo.
    void setSessionModel(session.backendId, model)
      .then((next) => {
        if (next) setModelOf(localId, next);
      })
      .catch((err) => set({ initError: String(err) }));
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
    // A draft holds nothing backend-side, so it goes at once.
    if (!session.backendId) {
      removeSession(id);
      toast.success(t('sidebar.deleted'));
      return;
    }
    // Flagged before the request: the row stays until the agent agrees, and
    // without a mark of its own it would look untouched while the reply is on
    // its way — which reads as nothing having happened.
    set((s) => ({ deletingIds: [...s.deletingIds, id] }));
    void requestDelete(session.backendId)
      .then(() => {
        removeSession(id);
        toast.success(t('sidebar.deleted'));
      })
      .catch((err) => {
        set({ initError: String(err) });
        toast.error(t('sidebar.deleteFailed'));
      })
      .finally(() => {
        set((s) => ({ deletingIds: s.deletingIds.filter((x) => x !== id) }));
      });
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
  // Where the view goes once the row is gone: `null` when the row that went was
  // not the one on screen, so the view stays where it is and there is nothing
  // more to ask for. An empty string means it was on screen and nothing is left.
  let movedTo: string | null = null;
  let removed = false;
  useSessionStore.setState((s) => {
    const remaining = s.sessions.filter((sess) => sess.id !== id);
    if (remaining.length === s.sessions.length) return s;
    removed = true;
    // The key is resolved before the row is filtered out: it comes from the
    // session itself, and the session is about to stop being there to ask.
    const key = draftKeyOf(s.sessions, id);
    const drafts = withoutDraft(s.drafts, key);
    const attachments = { ...s.attachments };
    delete attachments[key];
    if (s.activeId !== id) return { sessions: remaining, drafts, attachments };
    movedTo = remaining[0]?.id ?? '';
    return { sessions: remaining, activeId: movedTo, drafts, attachments };
  });
  if (!removed) return;
  saveDrafts(useSessionStore.getState().drafts);
  if (movedTo === null) return;
  // Nothing left to fall back to, so a draft: the sidebar is never a dead end.
  if (movedTo === '') {
    useSessionStore.getState().addSession();
    return;
  }
  // The neighbour's conversation is not on screen yet, so it is asked for the way
  // a click asks. Pointing the view at it alone would show it as one that nothing
  // was ever said in.
  useSessionStore.getState().select(movedTo);
}

// Whether a selector offers a model reference.
function offers(options: ModelOptions, value: string): boolean {
  return options.groups.some((group) =>
    group.choices.some((choice) => choice.value === value),
  );
}

// Writes a session's model selector, by this client's own id.
//
// A switch can be in flight while the view moves to another session, so the row
// is matched on the id it was read with rather than on whichever is active when
// the reply lands.
function setModelOf(localId: string, model: ModelOptions) {
  useSessionStore.setState((s) => ({
    sessions: s.sessions.map((sess) => (sess.id === localId ? { ...sess, model } : sess)),
  }));
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
  // A finished turn releases the first waiting follow-up as its own turn; that
  // turn's close releases the next, so the queue drains in arrival order.
  queueMicrotask(() => flushFollowups(sessionId));
}

// Applies the turn ends a `session/load` response reported, matched on the
// agent's own turn ids. A turn the response leaves out is one the agent is still
// running, so it stays open. The session's `updatedAt` is left alone: this is a
// conversation from the past, and stamping it now would make a stale row read as
// "just now".
function closeReplayedTurns(sessionId: string, turns: LoadedTurnEnd[]) {
  if (turns.length === 0) return;
  const ended = new Map(turns.map((turn) => [turn.turnId, turn.endReason]));
  useSessionStore.setState((s) => ({
    sessions: s.sessions.map((session) => {
      if (session.id !== sessionId) return session;
      let changed = false;
      const nextTurns = session.turns.map((turn) => {
        const reason =
          turn.backendTurnId === undefined ? undefined : ended.get(turn.backendTurnId);
        if (reason === undefined || turn.endReason !== undefined) return turn;
        changed = true;
        return { ...turn, endReason: reason, endedAt: Date.now() };
      });
      return changed ? { ...session, turns: nextTurns } : session;
    }),
  }));
}

// Hand the oldest waiting follow-up of a session to a new turn. One message per
// turn: the rest stay queued until that turn closes.
function flushFollowups(sessionId: string) {
  const state = useSessionStore.getState();
  const session = state.sessions.find((s) => s.id === sessionId);
  if (!session) return;
  const queue = session.followups ?? [];
  if (queue.length === 0) return;
  if (state.reopeningCount > 0 || isBusy(session)) return;
  useSessionStore.setState((s) => ({
    sessions: s.sessions.map((sess) =>
      sess.id === sessionId ? { ...sess, followups: queue.slice(1) } : sess,
    ),
  }));
  state.sendTurn(queue[0], sessionId);
}
