// @vitest-environment jsdom

// Tests the store against a mocked Tauri IPC: the live path (lazy backend
// creation, streaming updates folded into turns/steps, turn completion) and the
// reopen path (a remembered session's log replayed back into turns).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { Attachment, Session } from './types';
import {
  attachmentsFor,
  displayTitle,
  draftFor,
  isBusy,
  isReopening,
} from './sessionStore';
import { SESSION_NOT_FOUND, type Provider } from './acp';

const mocks = vi.hoisted(() => ({
  invoke: async (_cmd: string, _args?: Record<string, unknown>): Promise<unknown> => {
    throw new Error('invoke not configured');
  },
  notifyCb: null as ((event: { payload: unknown }) => void) | null,
  toast: { success: vi.fn(), error: vi.fn() },
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => mocks.invoke(cmd, args),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: (_event: string, cb: (e: { payload: unknown }) => void) => {
    mocks.notifyCb = cb;
    return Promise.resolve(() => undefined);
  },
}));

vi.mock('sonner', () => ({ toast: mocks.toast }));

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (err: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (err: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const reply = (id: number, result: unknown) => ({ jsonrpc: '2.0', id, result });

// The `_meta.srud` a `session/load` reply carries: every turn the session has
// finished, and why.
const turnsReply = (turns: Array<{ turnId: string; endReason: string }>) => ({
  _meta: { srud: { turns } },
});

// Boots a fresh store with a scripted backend: handshake succeeds, `session/new`
// hands out S1, S2, ... and `session/prompt` resolves only when the returned
// deferred settles. `session/load` likewise stays open until its deferred
// settles, because the log it hands over arrives before it does.
async function boot(
  options: {
    failDelete?: boolean;
    // Keeps `session/delete` open until its deferred settles, so a test can
    // watch the store while the agent has not answered yet.
    holdDelete?: boolean;
    loadFails?: { code: number; message: string };
    loadFailsFirst?: { code: number; message: string };
    listed?: Array<Record<string, unknown>>;
    // Makes `session/steer` refuse, as a turn that ended or sealed does.
    steerFails?: boolean;
    // Makes `session/regenerate` refuse, for the same reason.
    regenerateFails?: boolean;
    // The `configOptions` the agent hangs on each reply, as the wire spells them.
    newModel?: unknown;
    loadModel?: unknown;
    switched?: unknown;
  } = {},
) {
  vi.resetModules();
  mocks.notifyCb = null;
  const prompt = deferred<{ stopReason: string }>();
  // A regenerate answers only when its deferred settles, like a prompt: the
  // call returns the stop reason of the turn it ran.
  const regenerate = deferred<{ stopReason: string }>();
  // What the `session/load` reply carries beyond the model: how each replayed
  // turn ended. Resolved last: it is the last thing the client hears about the
  // conversation.
  const load = deferred<Record<string, unknown> | null>();
  const del = deferred<null>();
  const calls: string[] = [];
  const requested: Array<{ method: string; params: unknown }> = [];
  let created = 0;
  let loads = 0;
  mocks.invoke = async (cmd, args) => {
    if (cmd === 'default_cwd') return '/work';
    if (cmd === 'rpc_request') {
      const payload = args!.payload as { method: string; id: number; params: unknown };
      calls.push(payload.method);
      requested.push({ method: payload.method, params: payload.params });
      switch (payload.method) {
        case 'initialize':
          return reply(payload.id, { protocolVersion: 1, agentCapabilities: {} });
        case 'session/new':
          // Distinct ids so a test can tell a fallback session from the first.
          created += 1;
          return reply(payload.id, {
            sessionId: `S${created}`,
            ...(options.newModel === undefined ? {} : { configOptions: options.newModel }),
          });
        case 'session/prompt':
          return prompt.promise.then((r) => reply(payload.id, r));
        case 'session/list':
          return reply(payload.id, { sessions: options.listed ?? [] });
        case 'session/load': {
          // `loadFailsFirst` refuses one load and lets the rest through, so a test
          // can watch what the app does with the session it falls back to.
          const refusal =
            options.loadFails ?? (loads === 0 ? options.loadFailsFirst : undefined);
          loads += 1;
          if (refusal) {
            // Built from the module instance the store itself sees: a class
            // imported before the reset is a different object, and the store
            // tells refusals apart by its own `instanceof`.
            const { RpcFailure } = await import('./acp');
            throw new RpcFailure(refusal.code, refusal.message);
          }
          return load.promise.then((extra) =>
            reply(payload.id, {
              ...(options.loadModel === undefined ? {} : { configOptions: options.loadModel }),
              ...(extra ?? {}),
            }),
          );
        }
        case 'session/set_config_option':
          return reply(payload.id, {
            ...(options.switched === undefined ? {} : { configOptions: options.switched }),
          });
        case 'session/cancel':
          return reply(payload.id, {});
        case '_srud/unstable/session/set_title':
          return reply(payload.id, { title: 'Renamed' });
        case '_srud/unstable/session/steer':
          if (options.steerFails) throw new Error('steer refused');
          return reply(payload.id, {});
        case '_srud/unstable/session/regenerate':
          if (options.regenerateFails) throw new Error('regenerate refused');
          return regenerate.promise.then((r) => reply(payload.id, r));
        case 'session/delete':
          if (options.failDelete) throw new Error('delete refused');
          if (options.holdDelete) return del.promise.then(() => reply(payload.id, {}));
          return reply(payload.id, {});
      }
    }
    throw new Error(`unexpected invoke: ${cmd}`);
  };
  const { useSessionStore } = await import('./sessionStore');
  await useSessionStore.getState().init();
  const { useProviderStore } = await import('./providers');

  // `meta` is the `_meta.srud` the agent hangs on an update: the turn it belongs
  // to, and how that turn ended.
  const fire = (update: Record<string, unknown>, srud?: Record<string, unknown>) => {
    const backendId = useSessionStore.getState().sessions.find(
      (x) => x.id === useSessionStore.getState().activeId,
    )?.backendId;
    mocks.notifyCb?.({
      payload: {
        jsonrpc: '2.0',
        method: 'session/update',
        params: {
          sessionId: backendId ?? 'S-unknown',
          update,
          ...(srud ? { _meta: { srud } } : {}),
        },
      },
    });
  };
  const active = () => {
    const s = useSessionStore.getState();
    return s.sessions.find((x) => x.id === s.activeId) as Session;
  };
  const waitForBackend = async () => {
    await vi.waitFor(() => {
      expect(active().backendId).not.toBeNull();
    });
  };
  return {
    useSessionStore,
    useProviderStore,
    prompt,
    regenerate,
    load,
    deleteGate: del,
    fire,
    active,
    calls,
    requested,
    waitForBackend,
  };
}

beforeEach(() => {
  mocks.notifyCb = null;
  mocks.toast.success.mockClear();
  mocks.toast.error.mockClear();
});

describe('sessionStore live path', () => {
  it('init handshakes and opens a local draft without backend work', async () => {
    const { useSessionStore, active, calls } = await boot();
    const s = useSessionStore.getState();
    expect(s.initError).toBeNull();
    expect(s.sessions).toHaveLength(1);
    expect(active().backendId).toBeNull();
    expect(active().turns).toEqual([]);
    expect(calls).toContain('initialize');
    expect(calls).not.toContain('session/new');
  });

  it('clicking new chat on an empty chat opens nothing new', async () => {
    const { useSessionStore, calls } = await boot();
    // Boot already opens one draft, and it reads as "Start a new chat".
    expect(useSessionStore.getState().sessions).toHaveLength(1);
    useSessionStore.getState().addSession();
    useSessionStore.getState().addSession();
    expect(useSessionStore.getState().sessions).toHaveLength(1);
    expect(useSessionStore.getState().sessions[0].id).toBe(useSessionStore.getState().activeId);
    expect(calls).not.toContain('session/new');
  });

  it('clicking new chat away from a spoken-in session opens one draft', async () => {
    const { useSessionStore } = await boot();
    useSessionStore.getState().sendTurn('hi');
    expect(useSessionStore.getState().sessions[0].turns).toHaveLength(1);

    useSessionStore.getState().addSession();
    expect(useSessionStore.getState().sessions).toHaveLength(2);
    expect(useSessionStore.getState().sessions[0].turns).toEqual([]);
    expect(useSessionStore.getState().activeId).toBe(useSessionStore.getState().sessions[0].id);

    // The screen is an empty chat again, so the next click changes nothing.
    useSessionStore.getState().addSession();
    expect(useSessionStore.getState().sessions).toHaveLength(2);
  });

  it('the first message materializes the backend before prompting', async () => {
    const { useSessionStore, active, calls, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    await vi.waitFor(() => {
      expect(calls).toContain('session/prompt');
    });
    expect(active().backendId).toBe('S1');
    // Backend creation happens first, so no workspace exists for clicks alone.
    expect(calls.indexOf('session/new')).toBeLessThan(calls.indexOf('session/prompt'));
  });

  it('a turn that ends off screen marks the session unread until it is opened', async () => {
    const { useSessionStore, active, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    const spokenId = active().id;

    // The view moves to a draft, so the turn ends where nobody watches.
    useSessionStore.getState().addSession();
    expect(useSessionStore.getState().unreadIds).not.toContain(spokenId);
    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(useSessionStore.getState().unreadIds).toContain(spokenId);
    });

    useSessionStore.getState().select(spokenId);
    expect(useSessionStore.getState().unreadIds).not.toContain(spokenId);
  });

  it('a turn that ends on screen leaves nothing unread', async () => {
    const { useSessionStore, active, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    const spokenId = active().id;
    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
    expect(useSessionStore.getState().unreadIds).not.toContain(spokenId);
  });

  it('a message queued during a turn runs as the next turn once it ends', async () => {
    const { useSessionStore, active, calls, prompt, waitForBackend } = await boot();
    const promptsSoFar = () => calls.filter((m) => m === 'session/prompt').length;

    useSessionStore.getState().sendTurn('first');
    await waitForBackend();
    await vi.waitFor(() => {
      expect(promptsSoFar()).toBe(1);
    });

    // Typed while the turn runs: it waits its turn instead of starting a second
    // one alongside it.
    useSessionStore.getState().sendTurn('second');
    expect(active().followups).toEqual(['second']);
    expect(active().turns).toHaveLength(1);
    expect(promptsSoFar()).toBe(1);

    prompt.resolve({ stopReason: 'end_turn' });
    // The finished turn releases the waiting message as its own turn, in order.
    await vi.waitFor(() => {
      expect(active().turns).toHaveLength(2);
      expect(active().followups).toEqual([]);
    });
    expect(active().turns[1].userInput).toBe('second');
    await vi.waitFor(() => {
      expect(promptsSoFar()).toBe(2);
    });
  });

  it('an accepted steer leaves the waiting area without starting a turn', async () => {
    const { useSessionStore, active, calls, prompt, waitForBackend } = await boot();
    const promptsSoFar = () => calls.filter((m) => m === 'session/prompt').length;

    useSessionStore.getState().sendTurn('first');
    await waitForBackend();
    await vi.waitFor(() => {
      expect(promptsSoFar()).toBe(1);
    });
    useSessionStore.getState().sendTurn('second');
    expect(active().followups).toEqual(['second']);

    useSessionStore.getState().steerFollowup(0);
    await vi.waitFor(() => {
      expect(active().followups).toEqual([]);
    });
    // Steered into the running turn, not sent as a turn of its own.
    expect(promptsSoFar()).toBe(1);
    expect(useSessionStore.getState().initError).toBeNull();
    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
  });

  it('a refused steer keeps the message waiting instead of losing it', async () => {
    const { useSessionStore, active, calls, waitForBackend } = await boot({ steerFails: true });
    const promptsSoFar = () => calls.filter((m) => m === 'session/prompt').length;

    useSessionStore.getState().sendTurn('first');
    await waitForBackend();
    await vi.waitFor(() => {
      expect(promptsSoFar()).toBe(1);
    });
    useSessionStore.getState().sendTurn('second');
    expect(active().followups).toEqual(['second']);

    useSessionStore.getState().steerFollowup(0);
    await vi.waitFor(() => {
      expect(useSessionStore.getState().initError).not.toBeNull();
    });
    // The turn still runs and would not take it, so the message stays where the
    // user can still see it — a refusal is not a silent drop.
    expect(active().followups).toEqual(['second']);
    expect(promptsSoFar()).toBe(1);
  });

  it('a draft ignores backend updates until it materializes', async () => {
    const { active, fire } = await boot();
    expect(active().backendId).toBeNull();
    fire({ sessionUpdate: 'session_info_update', title: 'Should not stick' });
    expect(active().title).toBeNull();
  });

  it('takes the title the agent announces, with or without an open turn', async () => {
    const { useSessionStore, active, fire, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    expect(active().title).toBeNull();
    expect(displayTitle(active())).toBe('New session');

    // The agent names the session on the first prompt, before any turn output.
    fire({
      sessionUpdate: 'session_info_update',
      title: 'Why is the timestamp wrong?',
    });
    expect(active().title).toBe('Why is the timestamp wrong?');

    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });

    // A later rename applies the same way, including a clear.
    fire({ sessionUpdate: 'session_info_update', title: 'Chosen by hand' });
    expect(active().title).toBe('Chosen by hand');
    fire({ sessionUpdate: 'session_info_update', title: null });
    expect(active().title).toBeNull();
    expect(displayTitle(active())).toBe('New session');
  });

  it('takes a usage report and adds its breakdown to the session totals', async () => {
    const { useSessionStore, active, fire, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    fire(
      { sessionUpdate: 'usage_update', used: 1000, size: 128000 },
      {
        turnId: 't1',
        usage: {
          inputTokens: 900,
          outputTokens: 100,
          cachedInputTokens: 600,
          cacheWriteInputTokens: 0,
          reasoningOutputTokens: 0,
        },
      },
    );
    expect(active().usage).toEqual({
      used: 1000,
      size: 128000,
      inputTokens: 900,
      outputTokens: 100,
      cachedInputTokens: 600,
      cacheWriteInputTokens: 0,
      reasoningOutputTokens: 0,
    });

    // The next report replaces the occupancy — it is the newest reading of the
    // same window — and adds its share to the totals the cache rate is read from.
    // Fields it leaves out read as zero, so the earlier totals pass through.
    fire(
      { sessionUpdate: 'usage_update', used: 2500, size: 128000 },
      { turnId: 't1', usage: { inputTokens: 1600, cachedInputTokens: 1200 } },
    );
    expect(active().usage).toEqual({
      used: 2500,
      size: 128000,
      inputTokens: 2500,
      outputTokens: 100,
      cachedInputTokens: 1800,
      cacheWriteInputTokens: 0,
      reasoningOutputTokens: 0,
    });
  });

  it('a usage report without counts to read changes nothing', async () => {
    const { useSessionStore, active, fire, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    fire({ sessionUpdate: 'usage_update', used: 'many', size: 128000 });
    expect(active().usage).toBeUndefined();
  });

  it('renaming a draft stores the title locally without backend work', async () => {
    const { useSessionStore, active, calls } = await boot();
    const id = active().id;
    useSessionStore.getState().renameSession(id, '  Draft name  ');
    expect(active().title).toBe('Draft name');
    expect(calls).not.toContain('_srud/unstable/session/set_title');
  });

  it('a draft rename is pushed when the first message materializes it', async () => {
    const { useSessionStore, active, calls, requested, waitForBackend } = await boot();
    useSessionStore.getState().renameSession(active().id, 'Draft name');
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    await vi.waitFor(() => {
      expect(calls).toContain('_srud/unstable/session/set_title');
    });
    const pushed = requested.find((r) => r.method === '_srud/unstable/session/set_title');
    expect((pushed?.params as { title?: string })?.title).toBe('Draft name');
  });

  it('renaming a materialized session asks the agent and leaves the local title to its reply', async () => {
    const { useSessionStore, active, fire, calls, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    fire({ sessionUpdate: 'session_info_update', title: 'Derived' });

    useSessionStore.getState().renameSession(active().id, '  Chosen by hand  ');
    await vi.waitFor(() => {
      expect(calls).toContain('_srud/unstable/session/set_title');
    });
    // The agent echoes the accepted title back; nothing is written locally.
    expect(active().title).toBe('Derived');
  });

  it('deletes a draft without touching the backend', async () => {
    const { useSessionStore, calls } = await boot();
    // Something spoken in the open chat first: new chat is refused on an
    // empty one, so this is what makes a second draft possible at all.
    useSessionStore.getState().sendTurn('hi');
    useSessionStore.getState().addSession();
    expect(useSessionStore.getState().sessions).toHaveLength(2);
    const newest = useSessionStore.getState().sessions[0].id;
    useSessionStore.getState().deleteSession(newest);
    expect(useSessionStore.getState().sessions).toHaveLength(1);
    expect(calls).not.toContain('session/delete');
  });

  it('deletes a materialized session and falls back to another', async () => {
    const { useSessionStore, active, calls, requested, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    const backendId = active().backendId as string;
    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });

    const older = active().id;
    useSessionStore.getState().addSession();
    const newest = useSessionStore.getState().sessions[0].id;
    expect(newest).not.toBe(older);

    useSessionStore.getState().select(older);
    useSessionStore.getState().deleteSession(older);
    await vi.waitFor(() => {
      expect(calls).toContain('session/delete');
    });
    const del = requested.find((r) => r.method === 'session/delete');
    expect((del?.params as { sessionId?: string })?.sessionId).toBe(backendId);
    expect(useSessionStore.getState().sessions).toHaveLength(1);
    expect(useSessionStore.getState().sessions[0].id).toBe(newest);
  });

  it('deleting the last draft opens a fresh local one without backend work', async () => {
    const { useSessionStore, calls } = await boot();
    expect(useSessionStore.getState().sessions).toHaveLength(1);
    const only = useSessionStore.getState().sessions[0].id;

    useSessionStore.getState().deleteSession(only);
    const replacement = useSessionStore.getState().sessions[0].id;
    expect(replacement).not.toBe(only);
    expect(useSessionStore.getState().activeId).toBe(replacement);
    expect(useSessionStore.getState().sessions[0].backendId).toBeNull();
    expect(calls).not.toContain('session/delete');
    expect(calls).not.toContain('session/new');
  });

  it('deleting the last materialized session opens a fresh local draft', async () => {
    const { useSessionStore, calls, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(useSessionStore.getState().sessions[0].turns[0].endReason).toBe('completed');
    });
    const only = useSessionStore.getState().sessions[0].id;

    useSessionStore.getState().deleteSession(only);
    await vi.waitFor(() => {
      expect(calls).toContain('session/delete');
      expect(useSessionStore.getState().sessions[0].id).not.toBe(only);
    });
    // An empty sidebar would be a dead end, so a local replacement is opened —
    // still without backend work until its own first message.
    const replacement = useSessionStore.getState().sessions[0];
    expect(replacement.backendId).toBeNull();
    expect(useSessionStore.getState().activeId).toBe(replacement.id);
    expect(calls.filter((m) => m === 'session/new')).toHaveLength(1);
  });

  it('keeps a session visible when the agent refuses the delete', async () => {
    const { useSessionStore, prompt, waitForBackend } = await boot({ failDelete: true });
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    const id = useSessionStore.getState().sessions[0].id;
    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(useSessionStore.getState().sessions[0].turns[0].endReason).toBe('completed');
    });
    useSessionStore.getState().deleteSession(id);
    await vi.waitFor(() => {
      expect(useSessionStore.getState().initError).toContain('delete refused');
    });
    // The row is dropped only after the agent agrees, so a failed delete loses
    // nothing.
    expect(useSessionStore.getState().sessions).toHaveLength(1);
    expect(useSessionStore.getState().deletingIds).toEqual([]);
    expect(mocks.toast.error).toHaveBeenCalledWith('Could not delete the session.');
    expect(mocks.toast.success).not.toHaveBeenCalled();
  });

  it('says a delete is under way until the agent agrees', async () => {
    const { useSessionStore, active, deleteGate, waitForBackend } = await boot({
      holdDelete: true,
    });
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    const id = active().id;

    // Flagged the moment the delete is asked for: the row stays, dimmed with a
    // spinner, for as long as the agent keeps us waiting.
    useSessionStore.getState().deleteSession(id);
    expect(useSessionStore.getState().deletingIds).toEqual([id]);
    expect(useSessionStore.getState().sessions.some((s) => s.id === id)).toBe(true);

    deleteGate.resolve(null);
    await vi.waitFor(() => {
      expect(useSessionStore.getState().deletingIds).toEqual([]);
    });
    expect(useSessionStore.getState().sessions.some((s) => s.id === id)).toBe(false);
    expect(mocks.toast.success).toHaveBeenCalledWith('Session deleted');
  });

  it('marks a call pending until the update that carries its arguments', async () => {
    // The agent names a tool before the model has finished writing its
    // arguments, and for a tool taking a whole file that is the whole wait.
    const { useSessionStore, fire } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await vi.waitFor(() => {
      expect(useSessionStore.getState().sessions.some((s) => s.backendId !== null)).toBe(true);
    });
    const active = () => {
      const state = useSessionStore.getState();
      return state.sessions.find((s) => s.id === state.activeId)!;
    };

    fire({ sessionUpdate: 'tool_call', toolCallId: 'tc1', title: 'write' });
    expect(active().turns[0].steps[0].toolCalls[0]).toMatchObject({
      name: 'write',
      args: '',
      pending: true,
    });

    fire({
      sessionUpdate: 'tool_call_update',
      toolCallId: 'tc1',
      status: 'in_progress',
      rawInput: { path: 'x.rs', content: 'y' },
    });
    expect(active().turns[0].steps[0].toolCalls[0]).toMatchObject({
      args: '{"path":"x.rs","content":"y"}',
      pending: false,
    });
  });

  it('treats an update that names no status as having moved past pending', async () => {
    // `pending` is ACP's default and is left off the wire, so the common case is
    // the absence of the field. Reading that as still pending would leave the
    // mark spinning over a call that already has its arguments.
    const { useSessionStore, fire } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await vi.waitFor(() => {
      expect(useSessionStore.getState().sessions.some((s) => s.backendId !== null)).toBe(true);
    });
    const active = () => {
      const state = useSessionStore.getState();
      return state.sessions.find((s) => s.id === state.activeId)!;
    };

    fire({ sessionUpdate: 'tool_call', toolCallId: 'tc1', title: 'read', rawInput: { path: 'a' } });
    expect(active().turns[0].steps[0].toolCalls[0].pending).toBe(false);

    fire({
      sessionUpdate: 'tool_call_update',
      toolCallId: 'tc1',
      rawInput: { path: 'a' },
    });
    expect(active().turns[0].steps[0].toolCalls[0].pending).toBe(false);
  });

  it('describes a call further rather than adding a second row for it', async () => {
    // An agent announces a call and then describes it in full; a client that
    // treats every `tool_call` as a new call shows the reader two rows for one.
    const { useSessionStore, fire } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await vi.waitFor(() => {
      expect(useSessionStore.getState().sessions.some((s) => s.backendId !== null)).toBe(true);
    });
    const calls = () => {
      const state = useSessionStore.getState();
      return state.sessions.find((s) => s.id === state.activeId)!.turns[0].steps[0].toolCalls;
    };

    fire({ sessionUpdate: 'tool_call', toolCallId: 'tc1', title: 'read' });
    fire({
      sessionUpdate: 'tool_call',
      toolCallId: 'tc1',
      title: 'read',
      status: 'in_progress',
      rawInput: { path: 'a.rs' },
    });

    expect(calls()).toHaveLength(1);
    expect(calls()[0]).toMatchObject({ args: '{"path":"a.rs"}', pending: false });
  });

  it('streams agent text into the open turn and closes it on prompt completion', async () => {
    const { useSessionStore, active, fire, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    expect(active().turns[0].endReason).toBeUndefined();
    expect(active().turns[0].steps).toEqual([]);

    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'Hel' } });
    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'lo' } });
    expect(active().turns[0].steps[0].assistantText).toBe('Hello');

    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
  });

  it('folds reasoning into the step whose answer it precedes', async () => {
    const { useSessionStore, active, fire, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('why');
    await waitForBackend();

    fire({ sessionUpdate: 'agent_thought_chunk', content: { type: 'text', text: 'Let me ' } });
    fire({ sessionUpdate: 'agent_thought_chunk', content: { type: 'text', text: 'consider.' } });
    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'Because.' } });

    const steps = active().turns[0].steps;
    expect(steps).toHaveLength(1);
    expect(steps[0].thought).toBe('Let me consider.');
    expect(steps[0].assistantText).toBe('Because.');

    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
  });

  it('opens a new step for reasoning that follows a tool call', async () => {
    const { useSessionStore, active, fire, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('run it');
    await waitForBackend();

    fire({ sessionUpdate: 'agent_thought_chunk', content: { type: 'text', text: 'first' } });
    fire({ sessionUpdate: 'tool_call', toolCallId: 'tc1', title: 'shell', rawInput: { command: 'ls' } });
    fire({ sessionUpdate: 'agent_thought_chunk', content: { type: 'text', text: 'second' } });

    const steps = active().turns[0].steps;
    expect(steps).toHaveLength(2);
    expect(steps[0].thought).toBe('first');
    expect(steps[0].toolCalls[0].id).toBe('tc1');
    expect(steps[1].thought).toBe('second');
    expect(steps[1].toolCalls).toEqual([]);

    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
  });

  it('maps tool calls and their results onto steps', async () => {
    const { useSessionStore, active, fire, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('run it');
    await waitForBackend();

    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'Let me check. ' } });
    fire({
      sessionUpdate: 'tool_call',
      toolCallId: 'tc1',
      title: 'shell',
      rawInput: { command: 'ls' },
    });
    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'Done.' } });

    const steps = active().turns[0].steps;
    expect(steps).toHaveLength(2);
    expect(steps[0].assistantText).toBe('Let me check. ');
    expect(steps[0].toolCalls[0].name).toBe('shell');
    expect(steps[0].toolCalls[0].id).toBe('tc1');
    expect(steps[1].assistantText).toBe('Done.');
    expect(steps[1].toolCalls).toEqual([]);

    fire({
      sessionUpdate: 'tool_call_update',
      toolCallId: 'tc1',
      content: [{ type: 'content', content: { type: 'text', text: 'file.txt' } }],
    });
    expect(steps[0].toolCalls[0].result).toBe('file.txt');

    prompt.resolve({ stopReason: 'cancelled' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('interrupted');
    });
  });

  it('cancels the active turn and closes it as interrupted', async () => {
    const { useSessionStore, active, prompt, calls, waitForBackend } = await boot();
    const { isBusy } = await import('./sessionStore');
    useSessionStore.getState().sendTurn('go');
    await waitForBackend();

    // Busy from send until the prompt resolves, so the stop button is live.
    expect(isBusy(active())).toBe(true);

    useSessionStore.getState().stopTurn();
    await vi.waitFor(() => {
      expect(calls).toContain('session/cancel');
    });
    // Cancel only signals the token; the turn stays open until the prompt replies.
    expect(isBusy(active())).toBe(true);

    prompt.resolve({ stopReason: 'cancelled' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('interrupted');
    });
    expect(isBusy(active())).toBe(false);
  });

  it('does not cancel when no turn is running', async () => {
    const { useSessionStore, active, calls } = await boot();
    const { isBusy } = await import('./sessionStore');
    expect(isBusy(active())).toBe(false);

    useSessionStore.getState().stopTurn();
    // Flush the microtask queue and a macrotask, so a request that did fire
    // would have reached the mock before the assertion.
    await new Promise((r) => setTimeout(r, 0));
    expect(calls).not.toContain('session/cancel');
  });

  it('marks the turn as error when the prompt fails', async () => {
    const { useSessionStore, active, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    prompt.reject(new Error('boom'));
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('error');
      expect(useSessionStore.getState().initError).toContain('boom');
    });
  });

  it('surfaces handshake failure as initError without a session', async () => {
    vi.resetModules();
    mocks.invoke = async () => {
      throw new Error('-32603: initialize refused');
    };
    const { useSessionStore } = await import('./sessionStore');
    await useSessionStore.getState().init();
    expect(useSessionStore.getState().sessions).toEqual([]);
    expect(useSessionStore.getState().initError).toContain('initialize refused');
  });

  it('takes the turn id the agent names, so a turn is found by its updates', async () => {
    const { useSessionStore, active, fire, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'A' } }, { turnId: 'T1' });
    expect(active().turns[0].backendTurnId).toBe('T1');

    // A later turn's updates name a different turn, so they open their own rather
    // than continuing the one that is already on screen.
    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
    useSessionStore.getState().sendTurn('again');
    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'B' } }, { turnId: 'T2' });
    expect(active().turns).toHaveLength(2);
    expect(active().turns[1].backendTurnId).toBe('T2');
    expect(active().turns[1].steps[0].assistantText).toBe('B');
  });

  it('does not let the echoed user message overwrite what was sent', async () => {
    const { useSessionStore, active, fire, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('the original words');
    await waitForBackend();

    fire(
      { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'the original words' } },
      { turnId: 'T1' },
    );
    expect(active().turns[0].userInput).toBe('the original words');
    // The user's own message is not something the agent did, so it is not a step.
    expect(active().turns[0].steps).toEqual([]);

    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
  });

  it('leaves a turn open until the prompt response closes it', async () => {
    const { useSessionStore, active, fire, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('go');
    await waitForBackend();

    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'A' } }, { turnId: 'T1' });
    // The stream carries no turn boundary — a replayed chunk is the same update.
    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: '!' } }, { turnId: 'T1' });
    expect(active().turns[0].endReason).toBeUndefined();
    expect(active().turns[0].steps[0].assistantText).toBe('A!');

    prompt.resolve({ stopReason: 'cancelled' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('interrupted');
    });
    expect(active().turns[0].endedAt).toBeDefined();
  });
});


// The listing path: the agent is asked what sessions exist, and one is loaded
// when it is picked. A session outlives the run that made it, so this is where
// the ones from earlier runs come from.
describe('sessionStore listing path', () => {
  // What `session/list` answers with: one session from an earlier run, still on
  // disk, last active two hours ago.
  const LISTED = {
    sessionId: 'S-previous',
    cwd: '/work/previous',
    title: 'Why the build fails',
    updatedAt: new Date(Date.now() - 2 * 60 * 60 * 1000).toISOString(),
  };
  // An older one behind it. The agent lists the stored sessions newest first, so
  // a pair is what tells "the newest" apart from "the only one".
  const OLDER = {
    sessionId: 'S-older',
    cwd: '/work/older',
    title: 'Rename the crate',
    updatedAt: new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString(),
  };

  it('asks the agent what sessions exist and opens the newest', async () => {
    const { useSessionStore, calls, active } = await boot({ listed: [LISTED, OLDER] });

    expect(calls).toContain('session/list');
    const sessions = useSessionStore.getState().sessions;
    // No draft: there is a history to land on, and a blank one would sit on top of
    // it and take the screen.
    expect(sessions).toHaveLength(2);
    expect(sessions.every((s) => s.backendId !== null)).toBe(true);
    expect(active().backendId).toBe('S-previous');
    const stored = sessions.find((s) => s.backendId === 'S-previous');
    expect(stored?.title).toBe('Why the build fails');
    expect(stored?.cwd).toBe('/work/previous');
    // Its conversation is on its way rather than the row standing in for one that
    // nothing was ever said in.
    expect(calls).toContain('session/load');
  });

  it('starts once however many times it is asked', async () => {
    const { useSessionStore, calls } = await boot({ listed: [LISTED] });

    await useSessionStore.getState().init();

    // React's strict mode asks twice in development. A second start would ask the
    // agent for the conversation already on its way in, and every turn in it would
    // land twice.
    expect(calls.filter((m) => m === 'initialize')).toHaveLength(1);
    expect(calls.filter((m) => m === 'session/list')).toHaveLength(1);
    expect(calls.filter((m) => m === 'session/load')).toHaveLength(1);
  });

  it('opens with a draft alone when the agent has no sessions', async () => {
    const { useSessionStore, calls } = await boot();

    expect(calls).toContain('session/list');
    expect(useSessionStore.getState().sessions).toHaveLength(1);
    expect(useSessionStore.getState().sessions[0].backendId).toBeNull();
  });

  it('replays the conversation of the session it opens on', async () => {
    const { useSessionStore, calls, requested, fire, active, load } = await boot({
      listed: [LISTED, OLDER],
    });

    expect(active().id).toBe(useSessionStore.getState().sessions[0].id);
    // The directory it works in is named back, which is how the agent is told
    // which session's directory to confirm.
    await vi.waitFor(() => {
      expect(calls).toContain('session/load');
    });
    const req = requested.find((r) => r.method === 'session/load');
    expect(req?.params).toEqual({
      sessionId: 'S-previous',
      cwd: '/work/previous',
      mcpServers: [],
    });

    fire(
      { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'the question' } },
      { turnId: 'T1' },
    );
    fire(
      { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'the answer' } },
      { turnId: 'T1' },
    );
    load.resolve(turnsReply([{ turnId: 'T1', endReason: 'completed' }]));
    await vi.waitFor(() => {
      expect(active().turns[0]?.endReason).toBe('completed');
    });
    expect(active().turns).toHaveLength(1);
    expect(active().turns[0].userInput).toBe('the question');
    // The other one is untouched: what it holds is not on screen yet.
    const older = useSessionStore.getState().sessions.find((s) => s.backendId === 'S-older');
    expect(older?.loaded).toBe(false);
    expect(older?.turns).toEqual([]);
  });

  it('closes a replayed turn whose last record is a usage report', async () => {
    // A turn's usage report is recorded last, and a usage update is replayed only
    // when the model's window is known.
    const { fire, active, load } = await boot({ listed: [LISTED] });

    fire(
      { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'hi' } },
      { turnId: 'T1' },
    );
    fire(
      { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'hello' } },
      { turnId: 'T1' },
    );
    fire({ sessionUpdate: 'usage_update', used: 900, size: 128_000 }, { turnId: 'T1' });
    // Nothing in that stream says the turn is over.
    expect(isBusy(active())).toBe(true);

    load.resolve(turnsReply([{ turnId: 'T1', endReason: 'completed' }]));
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
    expect(isBusy(active())).toBe(false);
  });

  it('leaves a turn the response does not name open', async () => {
    // A turn left out of the list is one the agent is still running — a session
    // loaded while another client is mid-turn.
    const { useSessionStore, fire, active, load } = await boot({ listed: [LISTED] });

    fire(
      { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'go on' } },
      { turnId: 'T2' },
    );
    load.resolve(turnsReply([{ turnId: 'T1', endReason: 'completed' }]));
    await vi.waitFor(() => {
      expect(isReopening(useSessionStore.getState())).toBe(false);
    });
    expect(active().turns[0].endReason).toBeUndefined();
    expect(isBusy(active())).toBe(true);
  });

  it('applies the turn ends after the batched replay has landed', async () => {
    // Outside tests a replay is applied in animation frames, not in one store write
    // per update. The ends land after that flush: before it, the turns they name do
    // not exist yet.
    const { useSessionStore, fire, active, load } = await boot({ listed: [LISTED] });
    const saved = { vitest: process.env.VITEST, nodeEnv: process.env.NODE_ENV };
    delete process.env.VITEST;
    process.env.NODE_ENV = 'production';
    try {
      fire(
        { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'q' } },
        { turnId: 'T1' },
      );
      load.resolve(turnsReply([{ turnId: 'T1', endReason: 'completed' }]));
      await vi.waitFor(() => {
        expect(isReopening(useSessionStore.getState())).toBe(false);
      });
      expect(active().turns[0].endReason).toBe('completed');
    } finally {
      if (saved.vitest === undefined) delete process.env.VITEST;
      else process.env.VITEST = saved.vitest;
      if (saved.nodeEnv === undefined) delete process.env.NODE_ENV;
      else process.env.NODE_ENV = saved.nodeEnv;
    }
  });

  it('keeps the listed timestamp while the conversation replays', async () => {
    // A replayed update carries a conversation from the past. Restamping the
    // session as of now would make the sidebar read a two-hour-old session as
    // "just now" the moment it is clicked.
    const { fire, active, load } = await boot({ listed: [LISTED] });
    const listed = Date.parse(LISTED.updatedAt);
    expect(active().updatedAt).toBe(listed);

    fire(
      { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'the question' } },
      { turnId: 'T1' },
    );
    fire(
      { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'the answer' } },
      { turnId: 'T1' },
    );
    expect(active().turns).toHaveLength(1);
    expect(active().updatedAt).toBe(listed);

    load.resolve(turnsReply([{ turnId: 'T1', endReason: 'interrupted' }]));
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('interrupted');
    });
    // Closing a replayed turn is not activity: the row keeps the timestamp it
    // was listed with.
    expect(active().updatedAt).toBe(listed);
  });

  it('stamps a live update once the replayed conversation has arrived', async () => {
    // Past the load, updates are the session being active now, so the row should
    // read as recent.
    const { useSessionStore, fire, active, load } = await boot({ listed: [LISTED] });
    load.resolve(null);
    await vi.waitFor(() => {
      expect(isReopening(useSessionStore.getState())).toBe(false);
    });

    fire(
      { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'live' } },
      { turnId: 'T1' },
    );
    expect(active().updatedAt).toBeGreaterThan(Date.now() - 60_000);
  });

  it('loads the conversation of another listed session when it is picked', async () => {
    const { useSessionStore, calls, requested, load } = await boot({
      listed: [LISTED, OLDER],
    });
    const older = useSessionStore.getState().sessions.find((s) => s.backendId === 'S-older');

    useSessionStore.getState().select(older!.id);

    expect(useSessionStore.getState().activeId).toBe(older!.id);
    await vi.waitFor(() => {
      expect(calls.filter((m) => m === 'session/load')).toHaveLength(2);
    });
    const loads = requested.filter((r) => r.method === 'session/load');
    expect(loads[loads.length - 1]?.params).toEqual({
      sessionId: 'S-older',
      cwd: '/work/older',
      mcpServers: [],
    });
    load.resolve(null);
  });

  it('does not load the same conversation twice', async () => {
    // Loading again would replay it on top of itself, so every turn in it would
    // appear twice.
    const { useSessionStore, calls, fire, active, load } = await boot({ listed: [LISTED] });
    const listed = useSessionStore.getState().sessions.find((s) => s.backendId === 'S-previous');
    fire(
      { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'once' } },
      { turnId: 'T1' },
    );
    load.resolve(null);
    await vi.waitFor(() => {
      expect(active().turns).toHaveLength(1);
    });

    const loads = calls.filter((m) => m === 'session/load').length;
    // Away to a draft, which holds nothing to load, and back again.
    useSessionStore.getState().addSession();
    useSessionStore.getState().select(useSessionStore.getState().sessions[0].id);
    useSessionStore.getState().select(listed!.id);
    await new Promise((r) => setTimeout(r, 0));

    expect(calls.filter((m) => m === 'session/load')).toHaveLength(loads);
    expect(active().turns).toHaveLength(1);
  });

  it('refuses a prompt while a listed conversation is still arriving', async () => {
    const { useSessionStore, calls, active } = await boot({ listed: [LISTED] });
    await vi.waitFor(() => {
      expect(isReopening(useSessionStore.getState())).toBe(true);
    });

    useSessionStore.getState().sendTurn('too early');
    await new Promise((r) => setTimeout(r, 0));
    expect(calls).not.toContain('session/prompt');
    expect(active().turns).toEqual([]);
  });

  it('drops a session the agent will not name rather than showing it empty', async () => {
    // It was listed a moment ago and now cannot be loaded, so something removed
    // it. An empty conversation would be a lie about what the agent has.
    const { useSessionStore, active } = await boot({
      listed: [LISTED],
      loadFails: { code: SESSION_NOT_FOUND, message: 'no such session' },
    });

    // The row goes and a draft takes its place, so the sidebar is never a dead
    // end — waited for on the replacement, since the row count is one either way.
    await vi.waitFor(() => {
      expect(active().backendId).toBeNull();
    });
    expect(useSessionStore.getState().initError).toBeNull();
  });

  it('keeps a listed session and says so when loading it fails', async () => {
    // Unlike one that is gone, this is a fault: the conversation is still there
    // and a later attempt may well get it.
    const { useSessionStore } = await boot({
      listed: [LISTED],
      loadFails: { code: -32603, message: 'cannot load the session' },
    });

    await vi.waitFor(() => {
      expect(useSessionStore.getState().initError).toContain('cannot load the session');
    });
    expect(
      useSessionStore.getState().sessions.some((s) => s.backendId === 'S-previous'),
    ).toBe(true);
    expect(isReopening(useSessionStore.getState())).toBe(false);
  });

  it('loads the neighbour it falls back to rather than showing it empty', async () => {
    // The one on screen turned out to be unloadable, so the view moves to its
    // neighbour — and the neighbour's conversation is not on screen either. It has
    // to be asked for, or the row stands in for a conversation nothing was said in.
    const { calls, requested, active } = await boot({
      listed: [LISTED, OLDER],
      loadFailsFirst: { code: SESSION_NOT_FOUND, message: 'no such session' },
    });

    await vi.waitFor(() => {
      expect(active().backendId).toBe('S-older');
    });
    await vi.waitFor(() => {
      expect(calls.filter((m) => m === 'session/load')).toHaveLength(2);
    });
    const loads = requested.filter((r) => r.method === 'session/load');
    expect(loads[loads.length - 1]?.params).toEqual({
      sessionId: 'S-older',
      cwd: '/work/older',
      mcpServers: [],
    });
  });

  it('keeps the prompt closed until the fallback conversation has arrived', async () => {
    // The neighbour's load begins before the failed one has finished reporting, so
    // a flag cleared by the first to settle would open the prompt while the second
    // is still arriving and the two would interleave.
    const { useSessionStore, calls, active } = await boot({
      listed: [LISTED, OLDER],
      loadFailsFirst: { code: SESSION_NOT_FOUND, message: 'no such session' },
    });

    await vi.waitFor(() => {
      expect(active().backendId).toBe('S-older');
    });
    expect(isReopening(useSessionStore.getState())).toBe(true);

    useSessionStore.getState().sendTurn('too early');
    await new Promise((r) => setTimeout(r, 0));
    expect(calls).not.toContain('session/prompt');
  });

  it('deletes a listed session without opening it first', async () => {
    // The second one, which is not the session the app opens on, so nothing has
    // asked it for its conversation.
    const { useSessionStore, calls, requested } = await boot({ listed: [LISTED, OLDER] });
    const older = useSessionStore.getState().sessions.find((s) => s.backendId === 'S-older');
    expect(calls.filter((m) => m === 'session/load')).toHaveLength(1);

    useSessionStore.getState().deleteSession(older!.id);

    await vi.waitFor(() => {
      expect(calls).toContain('session/delete');
    });
    const req = requested.find((r) => r.method === 'session/delete');
    expect((req?.params as { sessionId?: string })?.sessionId).toBe('S-older');
    expect(
      useSessionStore.getState().sessions.some((s) => s.backendId === 'S-older'),
    ).toBe(false);
    // The session on screen is left alone, and keeps the conversation it has.
    expect(useSessionStore.getState().activeId).toBe(
      useSessionStore.getState().sessions[0].id,
    );
  });
});

// The `configOptions` of a reply, as the agent serializes them: one `model`
// option, grouped by provider. `configId` is the agent's own name for the
// option: ACP reserves the `model` category, not the id.
function modelOptions(currentValue: string, configId = 'model') {
  return [
    {
      id: configId,
      name: 'Model',
      category: 'model',
      type: 'select',
      currentValue,
      options: [
        {
          group: 'openai',
          name: 'OpenAI',
          options: [
            { value: 'openai/gpt-4o', name: 'GPT-4o' },
            { value: 'openai/gpt-4o-mini', name: 'GPT-4o mini' },
          ],
        },
        {
          group: 'deepseek',
          name: 'DeepSeek',
          options: [{ value: 'deepseek/chat', name: 'DeepSeek Chat' }],
        },
      ],
    },
  ];
}

// The provider catalog behind the model references `modelOptions` offers, as
// the settings panel would have loaded it.
function providerCatalog(defaultModel: string | null) {
  return {
    providers: [
      {
        id: 'openai',
        name: 'OpenAI',
        baseUrl: 'https://api.openai.com',
        hasKey: true,
        models: [
          { id: 'gpt-4o', name: 'GPT-4o', modalities: [] },
          { id: 'gpt-4o-mini', name: 'GPT-4o mini', modalities: [] },
        ],
      },
      {
        id: 'deepseek',
        name: 'DeepSeek',
        baseUrl: 'https://api.deepseek.com',
        hasKey: true,
        models: [{ id: 'chat', name: 'Chat', modalities: [] }],
      },
    ],
    defaultModel,
    loaded: true,
    error: null,
  };
}

// The `configOptions` of a reply that carries a thinking selector too: canonical
// levels, as the agent serializes a model that declares some.
function thinkingOptions(currentValue: string) {
  return {
    id: 'reasoning',
    name: 'Thinking',
    category: 'thought_level',
    type: 'select',
    currentValue,
    options: [
      { value: 'off', name: 'Off' },
      { value: 'medium', name: 'Medium' },
      { value: 'max', name: 'Max' },
    ],
  };
}

/** Both selectors, as one reply carries them. */
function sessionOptions(model: string, thinking: string) {
  return [...modelOptions(model), thinkingOptions(thinking)];
}

// The catalog behind the thinking tests: two models whose declarations differ,
// which is what makes a model switch reproject the draft's level.
const THINKING_PROVIDERS: Provider[] = [
  {
    id: 'openai',
    name: 'OpenAI',
    baseUrl: 'https://api.openai.com',
    hasKey: true,
    models: [
      {
        id: 'gpt-4o',
        name: 'GPT-4o',
        modalities: [],
        thinking: {
          default: 'medium',
          levels: { off: 'none', medium: 'medium', max: 'max' },
        },
      },
    ],
  },
  {
    id: 'deepseek',
    name: 'DeepSeek',
    baseUrl: 'https://api.deepseek.com',
    hasKey: true,
    models: [
      {
        id: 'chat',
        name: 'Chat',
        modalities: [],
        thinking: { default: 'low', levels: { low: 'low' } },
      },
    ],
  },
];

function thinkingCatalog() {
  return {
    providers: THINKING_PROVIDERS,
    defaultModel: 'openai/gpt-4o',
    loaded: true,
    error: null,
  };
}

describe('sessionStore model selection', () => {
  it('takes the model a new session was created with', async () => {
    // No catalog is configured in this test, so the draft has no selector to
    // show; the model arrives with the session the first prompt creates.
    const { useSessionStore, active, waitForBackend } = await boot({
      newModel: modelOptions('openai/gpt-4o'),
    });
    expect(active().model).toBeNull();

    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    expect(active().model?.currentValue).toBe('openai/gpt-4o');
    expect(active().model?.groups.map((g) => g.name)).toEqual(['OpenAI', 'DeepSeek']);
  });

  it('lets a draft choose a model and applies it to the session it becomes', async () => {
    const { useSessionStore, useProviderStore, active, calls, requested, waitForBackend } =
      await boot({
        newModel: modelOptions('openai/gpt-4o'),
        switched: modelOptions('deepseek/chat'),
      });
    useProviderStore.setState(providerCatalog('openai/gpt-4o'));

    // Before anything has been sent, the draft names the model it will run on.
    useSessionStore.getState().setModel('deepseek/chat');
    expect(active().backendId).toBeNull();
    expect(active().model?.currentValue).toBe('deepseek/chat');

    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    const switched = calls.indexOf('session/set_config_option');
    expect(switched).toBeGreaterThan(-1);
    // Switched before the prompt, so the first turn already runs on the choice.
    expect(switched).toBeLessThan(calls.indexOf('session/prompt'));
    expect(requested.find((r) => r.method === 'session/set_config_option')?.params).toEqual({
      sessionId: 'S1',
      configId: 'model',
      value: 'deepseek/chat',
    });
    expect(active().model?.currentValue).toBe('deepseek/chat');
  });

  it('switches through the id the agent gave the model option', async () => {
    const { useSessionStore, useProviderStore, requested, waitForBackend } = await boot({
      newModel: modelOptions('openai/gpt-4o', 'llm'),
      switched: modelOptions('deepseek/chat', 'llm'),
    });
    useProviderStore.setState(providerCatalog('openai/gpt-4o'));

    useSessionStore.getState().setModel('deepseek/chat');
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    // The option is the agent's, not the one this client happens to call it.
    expect(requested.find((r) => r.method === 'session/set_config_option')?.params).toEqual({
      sessionId: 'S1',
      configId: 'llm',
      value: 'deepseek/chat',
    });
  });

  it('keeps the agent model when the draft chose one it no longer offers', async () => {
    const { useSessionStore, useProviderStore, active, calls, waitForBackend } = await boot({
      newModel: modelOptions('openai/gpt-4o'),
    });
    useProviderStore.setState({
      ...providerCatalog('openai/gpt-4o'),
      providers: [
        {
          id: 'openai',
          name: 'OpenAI',
          baseUrl: 'https://api.openai.com',
          hasKey: true,
          models: [
            { id: 'gpt-4o', name: 'GPT-4o', modalities: [] },
            // The client's catalog lists a model the agent's session does not,
            // as when the agent's config changed after the catalog was read.
            { id: 'gpt-5', name: 'GPT-5', modalities: [] },
          ],
        },
      ],
    });

    useSessionStore.getState().setModel('openai/gpt-5');
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    expect(calls).not.toContain('session/set_config_option');
    expect(active().model?.currentValue).toBe('openai/gpt-4o');
  });

  it('leaves the picker absent when the agent offers none', async () => {
    const { useSessionStore, active, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    expect(active().model).toBeNull();
  });

  it('switches the model and takes the whole selector back', async () => {
    const { useSessionStore, active, requested, waitForBackend } = await boot({
      newModel: modelOptions('openai/gpt-4o'),
      switched: modelOptions('deepseek/chat'),
    });
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    useSessionStore.getState().setModel('deepseek/chat');

    await vi.waitFor(() => {
      expect(active().model?.currentValue).toBe('deepseek/chat');
    });
    const req = requested.find((r) => r.method === 'session/set_config_option');
    expect(req?.params).toEqual({
      sessionId: 'S1',
      configId: 'model',
      value: 'deepseek/chat',
    });
  });

  it('follows a model another client switched to', async () => {
    const { useSessionStore, active, fire, waitForBackend } = await boot({
      newModel: modelOptions('openai/gpt-4o'),
    });
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    // The agent broadcasts the whole selector, so the list comes with it.
    fire({ sessionUpdate: 'config_option_update', configOptions: modelOptions('deepseek/chat') });

    expect(active().model?.currentValue).toBe('deepseek/chat');
    // Session-level, like a rename: it lands whether or not a turn is open.
    expect(active().turns).toHaveLength(1);
  });

  it('takes the model a reopened session was left on, not its session-wide default', async () => {
    const listed = {
      sessionId: 'S-previous',
      cwd: '/work',
      title: 'Why the build fails',
      updatedAt: new Date().toISOString(),
    };
    const { load, active } = await boot({
      listed: [listed],
      loadModel: modelOptions('deepseek/chat'),
    });
    load.resolve(null);

    // The conversation is two things: its turns, and the model it was left on.
    // The reply carries the second, and only after `session/load` answers.
    await vi.waitFor(() => {
      expect(active().model?.currentValue).toBe('deepseek/chat');
    });
    expect(active().loaded).toBe(true);
  });

  it('does nothing on a session the agent gave no selector', async () => {
    const { useSessionStore, active, calls, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    useSessionStore.getState().setModel('deepseek/chat');
    await new Promise((r) => setTimeout(r, 0));

    expect(calls).not.toContain('session/set_config_option');
    expect(active().model).toBeNull();
  });
});

describe('sessionStore thinking selection', () => {
  it('takes the thinking selector a new session was created with', async () => {
    const { useSessionStore, active, waitForBackend } = await boot({
      newModel: sessionOptions('openai/gpt-4o', 'medium'),
    });
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    expect(active().thinking).toEqual({
      configId: 'reasoning',
      currentValue: 'medium',
      choices: [
        { value: 'off', name: 'Off' },
        { value: 'medium', name: 'Medium' },
        { value: 'max', name: 'Max' },
      ],
    });
  });

  it("records a draft's thinking level before its model has been switched", async () => {
    const { useSessionStore, useProviderStore, active } = await boot();
    useProviderStore.setState(thinkingCatalog());

    // A draft carries no model of its own until one is switched; the header
    // shows the catalog's default, so a level chosen there belongs to that
    // model and has to be recorded against it.
    expect(active().model).toBeNull();
    useSessionStore.getState().setThinking('max');

    expect(active().thinking?.currentValue).toBe('max');
  });

  it("applies a draft's thinking level to the session it becomes", async () => {
    const { useSessionStore, useProviderStore, active, calls, requested, waitForBackend } =
      await boot({
        newModel: sessionOptions('openai/gpt-4o', 'medium'),
        switched: sessionOptions('openai/gpt-4o', 'max'),
      });
    useProviderStore.setState(thinkingCatalog());

    useSessionStore.getState().setModel('openai/gpt-4o');
    useSessionStore.getState().setThinking('max');
    expect(active().backendId).toBeNull();
    expect(active().thinking?.currentValue).toBe('max');

    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    // The draft's level is applied before the prompt, so the first turn already
    // runs on it rather than on the model's default.
    const switched = requested.find((r) => r.method === 'session/set_config_option');
    expect(switched?.params).toEqual({
      sessionId: 'S1',
      configId: 'reasoning',
      value: 'max',
    });
    expect(calls.indexOf('session/set_config_option')).toBeLessThan(calls.indexOf('session/prompt'));
    expect(active().thinking?.currentValue).toBe('max');
  });

  it('switches thinking on a live session and takes both selectors back', async () => {
    const { useSessionStore, active, requested, waitForBackend } = await boot({
      newModel: sessionOptions('openai/gpt-4o', 'medium'),
      switched: sessionOptions('openai/gpt-4o', 'max'),
    });
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    useSessionStore.getState().setThinking('max');

    await vi.waitFor(() => {
      expect(active().thinking?.currentValue).toBe('max');
    });
    const req = requested.find((r) => r.method === 'session/set_config_option');
    expect(req?.params).toEqual({
      sessionId: 'S1',
      configId: 'reasoning',
      value: 'max',
    });
    // The reply carries both selectors, and the model one is refreshed with it.
    expect(active().model?.currentValue).toBe('openai/gpt-4o');
  });

  it('follows a thinking level another client switched to', async () => {
    const { useSessionStore, active, fire, waitForBackend } = await boot({
      newModel: sessionOptions('openai/gpt-4o', 'medium'),
    });
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    fire({
      sessionUpdate: 'config_option_update',
      configOptions: sessionOptions('openai/gpt-4o', 'off'),
    });

    expect(active().thinking?.currentValue).toBe('off');
    expect(active().turns).toHaveLength(1);
  });

  it('reprojects a draft level when the model it will run on changes', async () => {
    const { useSessionStore, useProviderStore, active } = await boot();
    useProviderStore.setState(thinkingCatalog());

    useSessionStore.getState().setModel('openai/gpt-4o');
    // A fresh draft starts on the model's declared default.
    expect(active().thinking?.currentValue).toBe('medium');
    useSessionStore.getState().setThinking('max');
    expect(active().thinking?.currentValue).toBe('max');

    // DeepSeek declares only `low`, so `max` falls back to its default rather
    // than being sent to an endpoint that would refuse it.
    useSessionStore.getState().setModel('deepseek/chat');
    expect(active().thinking?.currentValue).toBe('low');
    expect(active().thinking?.choices).toEqual([{ value: 'low', name: 'Low' }]);
  });

  it('does nothing on a session whose model declares no thinking', async () => {
    const { useSessionStore, active, calls, waitForBackend } = await boot({
      newModel: modelOptions('openai/gpt-4o'),
    });
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();

    useSessionStore.getState().setThinking('max');
    await new Promise((r) => setTimeout(r, 0));

    expect(calls).not.toContain('session/set_config_option');
    expect(active().thinking).toBeNull();
  });
});

// The draft each session's input box holds: switching sessions swaps what the
// box shows, and a send takes the draft of the session it was sent in.
describe('sessionStore drafts', () => {
  const LISTED = {
    sessionId: 'S-previous',
    cwd: '/work/previous',
    title: 'Why the build fails',
    updatedAt: new Date(Date.now() - 2 * 60 * 60 * 1000).toISOString(),
  };

  beforeEach(() => {
    localStorage.clear();
  });

  // One session that has been spoken in (so new chat opens a second one) and
  // the draft that follows it, with the first session's draft already typed.
  async function bootTwoSessions() {
    const booted = await boot();
    booted.useSessionStore.getState().sendTurn('hi');
    await booted.waitForBackend();
    booted.useSessionStore.getState().addSession();
    const first = booted.useSessionStore.getState().sessions[1];
    booted.useSessionStore.getState().setDraft(first.id, 'for the first');
    const second = booted.useSessionStore.getState().sessions[0];
    booted.useSessionStore.getState().setDraft(second.id, 'for the second');
    return { ...booted, first, second };
  }

  const stored = () => JSON.parse(localStorage.getItem('srud.drafts') ?? 'null');

  it('keeps a draft per session, so a switch swaps what the box shows', async () => {
    const { useSessionStore, first, second } = await bootTwoSessions();

    expect(draftFor(useSessionStore.getState(), second.id)).toBe('for the second');

    useSessionStore.getState().select(first.id);
    expect(draftFor(useSessionStore.getState(), first.id)).toBe('for the first');
    // What the second one holds is untouched by looking at the first.
    expect(draftFor(useSessionStore.getState(), second.id)).toBe('for the second');
  });

  it('files a draft under the backend id once the session has one', async () => {
    const { useSessionStore, first, second } = await bootTwoSessions();

    // The first session is materialized, so its draft is keyed by its backend
    // id; the second is a local draft, keyed by its local id.
    expect(stored()).toEqual({ S1: 'for the first', [second.id]: 'for the second' });
    expect(first.backendId).toBe('S1');
    expect(draftFor(useSessionStore.getState(), first.id)).toBe('for the first');
  });

  it('sending takes the draft of the session it was sent in, and only that one', async () => {
    const { useSessionStore, first, second, waitForBackend } = await bootTwoSessions();

    useSessionStore.getState().sendTurn('go');

    // The sent text is gone from the box it was sent in; the other session
    // keeps its draft.
    expect(draftFor(useSessionStore.getState(), second.id)).toBe('');
    expect(draftFor(useSessionStore.getState(), first.id)).toBe('for the first');

    await waitForBackend();
    expect(draftFor(useSessionStore.getState(), second.id)).toBe('');
    expect(stored()).toEqual({ S1: 'for the first' });
  });

  it('keeps a draft typed while the first turn is still in flight', async () => {
    // The backend session does not exist yet when the typing happens, so the
    // draft is filed under the local id and moves when the session
    // materializes.
    const { useSessionStore, active, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    useSessionStore.getState().setDraft(active().id, 'still typing');

    await waitForBackend();

    expect(draftFor(useSessionStore.getState(), active().id)).toBe('still typing');
    expect(stored()).toEqual({ S1: 'still typing' });

    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
  });

  it('keeps the draft when the store refuses the send', async () => {
    const { useSessionStore, active, calls } = await boot({ listed: [LISTED] });
    await vi.waitFor(() => {
      expect(isReopening(useSessionStore.getState())).toBe(true);
    });

    useSessionStore.getState().setDraft(active().id, 'too early');
    useSessionStore.getState().sendTurn('too early');
    await new Promise((r) => setTimeout(r, 0));

    expect(calls).not.toContain('session/prompt');
    expect(draftFor(useSessionStore.getState(), active().id)).toBe('too early');
  });

  it('drops the draft of a deleted session', async () => {
    const { useSessionStore, first, second } = await bootTwoSessions();

    // The materialized one, deleted while the draft is on screen: the delete
    // goes through the agent, and the draft goes with it.
    useSessionStore.getState().deleteSession(first.id);
    await vi.waitFor(() => {
      expect(stored()).toEqual({ [second.id]: 'for the second' });
    });

    expect(draftFor(useSessionStore.getState(), useSessionStore.getState().activeId)).toBe(
      'for the second',
    );
  });

  it('leaves no entry for a box that was cleared', async () => {
    const { useSessionStore, active } = await boot();

    useSessionStore.getState().setDraft(active().id, 'typed then cleared');
    useSessionStore.getState().setDraft(active().id, '');

    expect(useSessionStore.getState().drafts).toEqual({});
    expect(stored()).toEqual({});
  });

  it('restores the drafts of listed sessions and drops the rest', async () => {
    localStorage.setItem(
      'srud.drafts',
      JSON.stringify({
        'S-previous': 'leftover from last run',
        'S-gone': 'a session that no longer exists',
        's1000': 'a local draft that never left its run',
      }),
    );
    const { useSessionStore, active } = await boot({ listed: [LISTED] });

    expect(draftFor(useSessionStore.getState(), active().id)).toBe('leftover from last run');
    // Storage is rewritten with the live drafts.
    expect(stored()).toEqual({ 'S-previous': 'leftover from last run' });
  });
});

describe('sessionStore images', () => {
  const LISTED = {
    sessionId: 'S-previous',
    cwd: '/work/previous',
    title: 'A picture',
    updatedAt: new Date(Date.now() - 60 * 60 * 1000).toISOString(),
  };
  const SHOT: Attachment = { id: 'img1', mime: 'image/png', data: 'YWJj' };

  it('sends a pasted image with the prompt and takes it out of the box', async () => {
    const { useSessionStore, active, requested, waitForBackend } = await boot();
    useSessionStore.getState().setAttachments(active().id, [SHOT]);
    useSessionStore.getState().sendTurn('look at this', undefined, [SHOT]);

    // The image joins the turn, and the box it was pasted into is empty again.
    expect(active().turns[0].images).toEqual([SHOT]);
    expect(attachmentsFor(useSessionStore.getState(), active().id)).toEqual([]);

    await waitForBackend();
    await vi.waitFor(() => {
      const req = requested.find((r) => r.method === 'session/prompt');
      expect(req?.params).toMatchObject({
        prompt: [
          { type: 'image', mimeType: 'image/png', data: 'YWJj' },
          { type: 'text', text: 'look at this' },
        ],
      });
    });
  });

  it('sends an image on its own, with no text to carry it', async () => {
    const { useSessionStore, active, requested, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('', undefined, [SHOT]);
    expect(active().turns[0].images).toEqual([SHOT]);

    await waitForBackend();
    await vi.waitFor(() => {
      const req = requested.find((r) => r.method === 'session/prompt');
      expect(req?.params).toMatchObject({
        prompt: [{ type: 'image', mimeType: 'image/png', data: 'YWJj' }],
      });
    });
  });

  it('keeps each session its own images', async () => {
    const { useSessionStore, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    await waitForBackend();
    useSessionStore.getState().addSession();

    const state = () => useSessionStore.getState();
    const spokenIn = state().sessions[1];
    const draft = state().sessions[0];
    state().setAttachments(draft.id, [SHOT]);

    expect(attachmentsFor(state(), draft.id)).toEqual([SHOT]);
    // The other session's box is untouched by what was pasted here.
    expect(attachmentsFor(state(), spokenIn.id)).toEqual([]);
  });

  it('files images under the backend id once the session has one', async () => {
    const { useSessionStore, active, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('hi');
    // Pasted while the backend session is still being created, so the images
    // start under the local id and follow the session when it gets its own.
    useSessionStore.getState().setAttachments(active().id, [SHOT]);

    await waitForBackend();
    expect(active().backendId).toBe('S1');
    expect(Object.keys(useSessionStore.getState().attachments)).toEqual(['S1']);
    expect(attachmentsFor(useSessionStore.getState(), active().id)).toEqual([SHOT]);
  });

  it('replays a sent image onto the turn it was sent with', async () => {
    const { active, fire, load } = await boot({ listed: [LISTED] });
    fire(
      { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'see this' } },
      { turnId: 'T1' },
    );
    fire(
      {
        sessionUpdate: 'user_message_chunk',
        content: { type: 'image', mimeType: 'image/png', data: 'YWJj' },
      },
      { turnId: 'T1' },
    );
    fire(
      { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'yes' } },
      { turnId: 'T1' },
    );
    load.resolve(turnsReply([{ turnId: 'T1', endReason: 'completed' }]));

    await vi.waitFor(() => {
      expect(active().turns).toHaveLength(1);
    });
    expect(active().turns[0].userInput).toBe('see this');
    expect(active().turns[0].images).toEqual([
      expect.objectContaining({ mime: 'image/png', data: 'YWJj' }),
    ]);
  });
});

describe('sessionStore regenerate', () => {
  const SHOT: Attachment = { id: 'img1', mime: 'image/png', data: 'YWJj' };

  // One finished turn: sent, answered, and named T1 by the agent.
  async function answered(
    options: Parameters<typeof boot>[0] = {},
    input: { text?: string; images?: Attachment[] } = {},
  ) {
    const harness = await boot(options);
    const { useSessionStore, active, fire, prompt, waitForBackend } = harness;
    useSessionStore.getState().sendTurn(input.text ?? 'hi', undefined, input.images);
    await waitForBackend();
    fire(
      { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'first answer' } },
      { turnId: 'T1' },
    );
    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
    return harness;
  }

  it('asks the agent for the latest turn again, naming it', async () => {
    const { useSessionStore, active, requested } = await answered();

    useSessionStore.getState().regenerateTurn(active().turns[0]);
    await vi.waitFor(() => {
      expect(requested.some((r) => r.method === '_srud/unstable/session/regenerate')).toBe(true);
    });
    const req = requested.find((r) => r.method === '_srud/unstable/session/regenerate');
    expect(req?.params).toMatchObject({ sessionId: 'S1', turnId: 'T1' });
  });

  it('holds the message and the images while the answer goes', async () => {
    const { useSessionStore, active } = await answered(
      {},
      { text: 'look at this', images: [SHOT] },
    );

    useSessionStore.getState().regenerateTurn(active().turns[0]);

    // The answer goes the moment the request does, and the message it answered
    // stays in the same turn: a second turn holding the same words would show
    // them on screen twice.
    expect(active().turns).toHaveLength(1);
    const [asked] = active().turns;
    expect(asked.backendTurnId).toBeUndefined();
    expect(asked.userInput).toBe('look at this');
    expect(asked.images).toEqual([SHOT]);
    expect(asked.steps).toEqual([]);
    // Open again: the turn is being answered, and the answer is what will close
    // it. How it ended before belonged to the reply that was just taken back.
    expect(asked.endReason).toBeUndefined();
    expect(asked.endedAt).toBeUndefined();
    expect(isBusy(active())).toBe(true);
  });

  it('drops the discarded turn when the agent says so', async () => {
    const { useSessionStore, active, fire } = await answered({}, { images: [SHOT] });

    useSessionStore.getState().regenerateTurn(active().turns[0]);
    // A discard rides on the empty update ACP has for the purpose.
    fire({ sessionUpdate: 'session_info_update' }, { turnDiscarded: 'T1' });

    // The message and its images stay; the answer goes.
    const turns = active().turns;
    expect(turns).toHaveLength(1);
    expect(turns[0].userInput).toBe('hi');
    expect(turns[0].images).toEqual([SHOT]);
    expect(turns[0].steps).toEqual([]);
    expect(turns[0].backendTurnId).toBeUndefined();
    // And it is not mistaken for a title change, which the update looks like.
    expect(active().title).toBeNull();
  });

  it('streams the new answer into the turn it replaced', async () => {
    const { useSessionStore, active, fire, regenerate } = await answered();

    useSessionStore.getState().regenerateTurn(active().turns[0]);
    fire({ sessionUpdate: 'session_info_update' }, { turnDiscarded: 'T1' });
    fire(
      { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'second' } },
      { turnId: 'T2' },
    );

    const turns = active().turns;
    expect(turns).toHaveLength(1);
    expect(turns[0].backendTurnId).toBe('T2');
    expect(turns[0].userInput).toBe('hi');
    expect(turns[0].steps[0].assistantText).toBe('second');

    regenerate.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('completed');
    });
  });

  it('closes an interrupted re-answer as interrupted', async () => {
    const { useSessionStore, active, regenerate, waitForBackend } = await answered();

    useSessionStore.getState().regenerateTurn(active().turns[0]);
    await vi.waitFor(() => {
      expect(isBusy(active())).toBe(true);
    });
    regenerate.resolve({ stopReason: 'cancelled' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('interrupted');
    });
    // Stopping the re-answer leaves what it was asked again in place.
    const turns = active().turns;
    expect(turns).toHaveLength(1);
    expect(turns[0].userInput).toBe('hi');
    void waitForBackend;
  });

  it('puts the turn back untouched when the agent refuses', async () => {
    const { useSessionStore, active } = await answered({ regenerateFails: true });
    const before = active().turns;

    useSessionStore.getState().regenerateTurn(before[0]);
    // Everything the request does happens before it is sent, so the message is
    // already back to being unanswered before the agent's answer can arrive.
    expect(active().turns).toHaveLength(1);
    expect(active().turns[0].steps).toEqual([]);
    await vi.waitFor(() => {
      expect(useSessionStore.getState().initError).toContain('regenerate refused');
    });

    // A refusal comes before the agent has recorded anything, so the answer it
    // was asked to replace is still the agent's latest turn. The turn on screen
    // takes back everything it gave up, and the refusal is reported beside it
    // rather than as an empty bubble over an answer that never ran.
    const turns = active().turns;
    expect(turns).toHaveLength(1);
    expect(turns[0].id).toBe(before[0].id);
    expect(turns[0].backendTurnId).toBe('T1');
    expect(turns[0].endReason).toBe('completed');
    expect(turns[0].userInput).toBe('hi');
    expect(isBusy(active())).toBe(false);
  });

  it('refuses to ask again while a turn is running', async () => {
    const { useSessionStore, active, prompt, calls, waitForBackend } = await answered();
    useSessionStore.getState().sendTurn('and another');
    await vi.waitFor(() => {
      expect(isBusy(active())).toBe(true);
    });

    useSessionStore.getState().regenerateTurn(active().turns[0]);
    await new Promise((r) => setTimeout(r, 0));
    expect(calls).not.toContain('_srud/unstable/session/regenerate');

    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(isBusy(active())).toBe(false);
    });
    void waitForBackend;
  });

  it('asks again only for the latest turn', async () => {
    const { useSessionStore, active, prompt, calls } = await answered();
    useSessionStore.getState().sendTurn('and another');
    await vi.waitFor(() => {
      expect(active().turns).toHaveLength(2);
    });
    prompt.resolve({ stopReason: 'end_turn' });
    await vi.waitFor(() => {
      expect(active().turns[1].endReason).toBe('completed');
    });

    useSessionStore.getState().regenerateTurn(active().turns[0]);
    await new Promise((r) => setTimeout(r, 0));
    expect(calls).not.toContain('_srud/unstable/session/regenerate');
  });
});
