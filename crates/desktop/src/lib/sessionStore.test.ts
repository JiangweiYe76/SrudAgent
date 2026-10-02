// Tests the store's live path against a mocked Tauri IPC: lazy backend
// creation, streaming updates folded into turns/steps, and turn completion.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { Session } from './types';
import { displayTitle } from './sessionStore';

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
// deferred settles.
async function boot(options: { failDelete?: boolean } = {}) {
  vi.resetModules();
  mocks.notifyCb = null;
  const prompt = deferred<{ stopReason: string }>();
  const calls: string[] = [];
  const requested: Array<{ method: string; params: unknown }> = [];
  let created = 0;
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

  const fire = (update: Record<string, unknown>) => {
    const backendId = useSessionStore.getState().sessions.find(
      (x) => x.id === useSessionStore.getState().activeId,
    )?.backendId;
    mocks.notifyCb?.({
      payload: {
        jsonrpc: '2.0',
        method: 'session/update',
        params: { sessionId: backendId ?? 'S-unknown', update },
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
  return { useSessionStore, prompt, fire, active, calls, requested, waitForBackend };
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
});
