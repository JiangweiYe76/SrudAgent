// @vitest-environment jsdom

// Tests the store against a mocked Tauri IPC: the live path (lazy backend
// creation, streaming updates folded into turns/steps, turn completion) and the
// reopen path (a remembered session's log replayed back into turns).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { Session } from './types';
import { displayTitle, isReopening } from './sessionStore';
import { SESSION_NOT_FOUND } from './acp';

const mocks = vi.hoisted(() => ({
  invoke: async (_cmd: string, _args?: Record<string, unknown>): Promise<unknown> => {
    throw new Error('invoke not configured');
  },
  notifyCb: null as ((event: { payload: unknown }) => void) | null,
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

// Boots a fresh store with a scripted backend: handshake succeeds, `session/new`
// hands out S1, S2, ... and `session/prompt` resolves only when the returned
// deferred settles. `session/load` likewise stays open until its deferred
// settles, because the log it hands over arrives before it does.
async function boot(
  options: {
    failDelete?: boolean;
    loadFails?: { code: number; message: string };
    loadFailsFirst?: { code: number; message: string };
    listed?: Array<Record<string, unknown>>;
  } = {},
) {
  vi.resetModules();
  mocks.notifyCb = null;
  const prompt = deferred<{ stopReason: string }>();
  const load = deferred<null>();
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
          return reply(payload.id, { sessionId: `S${created}` });
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
          return load.promise.then(() => reply(payload.id, {}));
        }
        case 'session/cancel':
          return reply(payload.id, {});
        case '_srud/unstable/session/set_title':
          return reply(payload.id, { title: 'Renamed' });
        case 'session/delete':
          if (options.failDelete) throw new Error('delete refused');
          return reply(payload.id, {});
      }
    }
    throw new Error(`unexpected invoke: ${cmd}`);
  };
  const { useSessionStore } = await import('./sessionStore');
  await useSessionStore.getState().init();

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
  return { useSessionStore, prompt, load, fire, active, calls, requested, waitForBackend };
}

beforeEach(() => {
  mocks.notifyCb = null;
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

  it('clicking new chat creates only local drafts', async () => {
    const { useSessionStore, calls } = await boot();
    useSessionStore.getState().addSession();
    useSessionStore.getState().addSession();
    expect(useSessionStore.getState().sessions).toHaveLength(3);
    expect(calls).not.toContain('session/new');
    expect(
      useSessionStore.getState().sessions.every((s) => s.backendId === null),
    ).toBe(true);
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
    fire({ sessionUpdate: 'tool_call', toolCallId: 'tc1', title: 'bash', rawInput: { command: 'ls' } });
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
      title: 'bash',
      rawInput: { command: 'ls' },
    });
    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'Done.' } });

    const steps = active().turns[0].steps;
    expect(steps).toHaveLength(2);
    expect(steps[0].assistantText).toBe('Let me check. ');
    expect(steps[0].toolCalls[0].name).toBe('bash');
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
      throw new Error('-32603: agent is not configured');
    };
    const { useSessionStore } = await import('./sessionStore');
    await useSessionStore.getState().init();
    expect(useSessionStore.getState().sessions).toEqual([]);
    expect(useSessionStore.getState().initError).toContain('not configured');
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

  it('closes a turn on the update that says how it ended', async () => {
    const { useSessionStore, active, fire, prompt, waitForBackend } = await boot();
    useSessionStore.getState().sendTurn('go');
    await waitForBackend();

    fire({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'A' } }, { turnId: 'T1' });
    fire(
      { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: '!' } },
      { turnId: 'T1', turnEndReason: 'interrupted' },
    );
    expect(active().turns[0].endReason).toBe('interrupted');
    expect(active().turns[0].steps[0].assistantText).toBe('A!');
    expect(active().turns[0].endedAt).toBeDefined();

    prompt.resolve({ stopReason: 'cancelled' });
    await vi.waitFor(() => {
      expect(active().turns[0].endReason).toBe('interrupted');
    });
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
      { turnId: 'T1', turnEndReason: 'completed' },
    );
    load.resolve(null);
    await vi.waitFor(() => {
      expect(active().turns).toHaveLength(1);
    });
    expect(active().turns[0].userInput).toBe('the question');
    expect(active().turns[0].endReason).toBe('completed');
    // The other one is untouched: what it holds is not on screen yet.
    const older = useSessionStore.getState().sessions.find((s) => s.backendId === 'S-older');
    expect(older?.loaded).toBe(false);
    expect(older?.turns).toEqual([]);
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
      { turnId: 'T1', turnEndReason: 'completed' },
    );
    expect(active().turns).toHaveLength(1);
    expect(active().updatedAt).toBe(listed);

    load.resolve(null);
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
      { turnId: 'T1', turnEndReason: 'completed' },
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
