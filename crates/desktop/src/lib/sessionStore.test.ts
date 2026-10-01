// Tests the store's live path against a mocked Tauri IPC: the handshake,
// streaming updates folded into turns/steps, and turn completion.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { Session } from './types';

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

// Boots a fresh store with a scripted backend: handshake succeeds, and the
// first `session/prompt` resolves only when the returned deferred settles.
async function boot() {
  vi.resetModules();
  mocks.notifyCb = null;
  const prompt = deferred<{ stopReason: string }>();
  mocks.invoke = async (cmd, args) => {
    if (cmd === 'default_cwd') return '/work';
    if (cmd === 'rpc_request') {
      const payload = args!.payload as { method: string; id: number };
      switch (payload.method) {
        case 'initialize':
          return reply(payload.id, { protocolVersion: 1, agentCapabilities: {} });
        case 'session/new':
          return reply(payload.id, { sessionId: 'S1' });
        case 'session/prompt':
          return prompt.promise.then((r) => reply(payload.id, r));
      }
    }
    throw new Error(`unexpected invoke: ${cmd}`);
  };
  const { useSessionStore } = await import('./sessionStore');
  await useSessionStore.getState().init();

  const fire = (update: Record<string, unknown>) => {
    mocks.notifyCb?.({
      payload: {
        jsonrpc: '2.0',
        method: 'session/update',
        params: { sessionId: 'S1', update },
      },
    });
  };
  const active = () => {
    const s = useSessionStore.getState();
    return s.sessions.find((x) => x.id === s.activeId) as Session;
  };
  return { useSessionStore, prompt, fire, active };
}

beforeEach(() => {
  mocks.notifyCb = null;
});

describe('sessionStore live path', () => {
  it('init handshakes and opens one real session', async () => {
    const { useSessionStore, active } = await boot();
    const s = useSessionStore.getState();
    expect(s.initError).toBeNull();
    expect(s.sessions).toHaveLength(1);
    expect(active().id).toBe('S1');
    expect(active().turns).toEqual([]);
  });

  it('streams agent text into the open turn and closes it on prompt completion', async () => {
    const { useSessionStore, active, fire, prompt } = await boot();
    useSessionStore.getState().sendTurn('hi');
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
    const { useSessionStore, active, fire, prompt } = await boot();
    useSessionStore.getState().sendTurn('why');

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
    const { useSessionStore, active, fire, prompt } = await boot();
    useSessionStore.getState().sendTurn('run it');

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
    const { useSessionStore, active, fire, prompt } = await boot();
    useSessionStore.getState().sendTurn('run it');

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

  it('marks the turn as error when the prompt fails', async () => {
    const { useSessionStore, active, prompt } = await boot();
    useSessionStore.getState().sendTurn('hi');
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
