// @vitest-environment node
import { describe, it, expect, vi } from 'vitest';

// The module reaches the agent through the Tauri IPC bridge, which does not
// exist in a test process; the mock stands in for it. The parsing under test is
// pure.
//
// The bridge answers whatever the current test parked in `bridge.reply`, so a
// test can drive one call to the agent and read what the parser made of it.
const bridge = vi.hoisted(() => ({ reply: {} as Record<string, unknown> }));
vi.mock('@tauri-apps/api/core', () => ({
  invoke: async (_cmd: string, args?: unknown) => {
    const { id } = (args as { payload: { id: number } }).payload;
    return { jsonrpc: '2.0', id, result: bridge.reply };
  },
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));

import {
  listProviders,
  loadSession,
  parseModelOptions,
  parseThinkingOptions,
  UNAVAILABLE_GROUP,
} from './acp';

/** One `providerId/modelId` select option, as the agent builds it. */
const choice = (value: string, name: string) => ({ value, name });

/** The `configOptions` of a response, built the way SrudAgent serializes it. */
function options(options: unknown[], currentValue = 'openai/gpt-4o') {
  return [
    {
      id: 'model',
      name: 'Model',
      description: 'The provider and model this conversation uses.',
      category: 'model',
      type: 'select',
      currentValue,
      options,
    },
  ];
}

describe('parseModelOptions', () => {
  it('reads the grouped shape the agent sends', () => {
    const parsed = parseModelOptions(
      options([
        {
          group: 'openai',
          name: 'OpenAI',
          options: [
            choice('openai/gpt-4o', 'GPT-4o'),
            choice('openai/mini', 'Mini'),
          ],
        },
        {
          group: 'deepseek',
          name: 'DeepSeek',
          options: [choice('deepseek/chat', 'Chat')],
        },
      ]),
    );

    expect(parsed).toEqual({
      configId: 'model',
      currentValue: 'openai/gpt-4o',
      groups: [
        {
          id: 'openai',
          name: 'OpenAI',
          choices: [
            { value: 'openai/gpt-4o', name: 'GPT-4o' },
            { value: 'openai/mini', name: 'Mini' },
          ],
        },
        {
          id: 'deepseek',
          name: 'DeepSeek',
          choices: [{ value: 'deepseek/chat', name: 'Chat' }],
        },
      ],
    });
  });

  // ACP types the set of values as an untagged union — a flat list or a list of
  // groups — so which one arrived is decided by the entries, not by a tag.
  it('reads a flat list as one unnamed group', () => {
    const parsed = parseModelOptions(
      options([
        choice('openai/gpt-4o', 'GPT-4o'),
        choice('openai/mini', 'Mini'),
      ]),
    );

    expect(parsed?.groups).toEqual([
      {
        id: '',
        name: '',
        choices: [
          { value: 'openai/gpt-4o', name: 'GPT-4o' },
          { value: 'openai/mini', name: 'Mini' },
        ],
      },
    ]);
  });

  it('reports no selector when the agent offered none', () => {
    expect(parseModelOptions(undefined)).toBeNull();
    expect(parseModelOptions([])).toBeNull();
    // Another agent's own options, with no model among them.
    expect(
      parseModelOptions([{ id: 'mode', type: 'select', currentValue: 'code' }]),
    ).toBeNull();
  });

  it('accepts an option named only by its category', () => {
    const parsed = parseModelOptions([
      {
        id: 'which-model',
        name: 'Model',
        category: 'model',
        type: 'select',
        currentValue: 'openai/gpt-4o',
        options: [choice('openai/gpt-4o', 'GPT-4o')],
      },
    ]);

    expect(parsed?.configId).toBe('which-model');
    expect(parsed?.currentValue).toBe('openai/gpt-4o');
  });

  it('keeps an empty selection as an empty selection', () => {
    // The session has been given no model — no default was chosen and nobody
    // has picked one for it. That is not a value that has gone missing, so it
    // must not be given an entry of its own in the list.
    const parsed = parseModelOptions(
      options([choice('openai/gpt-4o', 'GPT-4o')], ''),
    );

    expect(parsed?.currentValue).toBe('');
    expect(parsed?.groups).toEqual([
      {
        id: '',
        name: '',
        choices: [{ value: 'openai/gpt-4o', name: 'GPT-4o' }],
      },
    ]);
  });

  it('keeps a selection that no provider offers any more', () => {
    // The agent reports the session's model as it is, so a deleted provider
    // leaves a value the list does not hold. Dropping it would show the session
    // as running on some other model.
    const parsed = parseModelOptions(
      options([choice('openai/gpt-4o', 'GPT-4o')], 'gone/model'),
    );

    expect(parsed?.currentValue).toBe('gone/model');
    expect(parsed?.groups).toEqual([
      {
        id: '',
        name: '',
        choices: [{ value: 'openai/gpt-4o', name: 'GPT-4o' }],
      },
      {
        id: UNAVAILABLE_GROUP,
        name: '',
        choices: [{ value: 'gone/model', name: 'gone/model' }],
      },
    ]);
  });

  it('does not double up the group the agent already added for a stale value', () => {
    const parsed = parseModelOptions(
      options(
        [
          {
            group: 'openai',
            name: 'OpenAI',
            options: [choice('openai/gpt-4o', 'GPT-4o')],
          },
          {
            group: UNAVAILABLE_GROUP,
            name: 'Unavailable',
            options: [choice('gone/model', 'gone/model')],
          },
        ],
        'gone/model',
      ),
    );

    expect(
      parsed?.groups.filter((g) => g.id === UNAVAILABLE_GROUP),
    ).toHaveLength(1);
  });

  it('skips entries that are not choices', () => {
    const parsed = parseModelOptions(
      options([
        choice('openai/gpt-4o', 'GPT-4o'),
        { name: 'no value' },
        null,
        7,
      ]),
    );

    expect(parsed?.groups[0].choices).toEqual([
      { value: 'openai/gpt-4o', name: 'GPT-4o' },
    ]);
  });

  it('falls back to the value when a choice has no name', () => {
    const parsed = parseModelOptions(options([{ value: 'openai/gpt-4o' }]));

    expect(parsed?.groups[0].choices).toEqual([
      { value: 'openai/gpt-4o', name: 'openai/gpt-4o' },
    ]);
  });
});

describe('parseThinkingOptions', () => {
  // A thinking option as the agent serializes it: canonical levels with their
  // display labels.
  const thinking = (
    currentValue = 'medium',
    id = 'reasoning',
    category?: string,
  ) => ({
    ...(category === undefined ? {} : { category }),
    id,
    name: 'Thinking',
    type: 'select',
    currentValue,
    options: [
      { value: 'off', name: 'Off' },
      { value: 'medium', name: 'Medium' },
      { value: 'max', name: 'Max' },
    ],
  });

  it('reads the option and its canonical choices', () => {
    expect(parseThinkingOptions([thinking()])).toEqual({
      configId: 'reasoning',
      currentValue: 'medium',
      choices: [
        { value: 'off', name: 'Off' },
        { value: 'medium', name: 'Medium' },
        { value: 'max', name: 'Max' },
      ],
    });
  });

  it('accepts an option named only by its category', () => {
    const parsed = parseThinkingOptions([
      thinking('max', 'effort', 'thought_level'),
    ]);

    expect(parsed?.configId).toBe('effort');
    expect(parsed?.currentValue).toBe('max');
  });

  it('reports no selector for a model that declares no thinking', () => {
    expect(parseThinkingOptions(undefined)).toBeNull();
    expect(parseThinkingOptions([])).toBeNull();
    // A model with no declared levels has no choices to offer.
    expect(
      parseThinkingOptions([{ id: 'reasoning', type: 'select', options: [] }]),
    ).toBeNull();
  });
});

describe('loadSession', () => {
  it('reads how each replayed turn ended off the response', async () => {
    bridge.reply = {
      _meta: {
        srud: {
          turns: [
            { turnId: 'T1', endReason: 'completed' },
            { turnId: 'T2', endReason: 'interrupted' },
          ],
        },
      },
    };

    const loaded = await loadSession('S1', '/work');
    expect(loaded.turns).toEqual([
      { turnId: 'T1', endReason: 'completed' },
      { turnId: 'T2', endReason: 'interrupted' },
    ]);
  });

  it('drops a turn end it cannot read rather than guessing at one', async () => {
    // A dropped entry leaves its turn open, which reads as running.
    bridge.reply = {
      _meta: {
        srud: {
          turns: [
            { turnId: 'T1', endReason: 'completed' },
            { turnId: 7, endReason: 'completed' },
            { turnId: 'T2', endReason: 'vanished' },
          ],
        },
      },
    };

    expect((await loadSession('S1')).turns).toEqual([
      { turnId: 'T1', endReason: 'completed' },
    ]);
  });

  it('reports no turn ends when the response carries none', async () => {
    bridge.reply = {};
    expect((await loadSession('S1')).turns).toEqual([]);
  });
});

describe('listProviders', () => {
  // Parks a catalog for the bridge to answer with, then reads it back.
  async function read(models: unknown[]) {
    bridge.reply = {
      providers: [
        {
          id: 'p',
          name: 'P',
          baseUrl: 'https://p.example/v1',
          hasKey: false,
          models,
        },
      ],
      default: null,
    };
    return (await listProviders()).providers[0].models;
  }

  it('reads a model´s modalities in a stable order', async () => {
    // Sorted the way the protocol lists them, not the way they arrived.
    const models = await read([{ id: 'omni', modalities: ['video', 'image'] }]);
    expect(models[0].modalities).toEqual(['image', 'video']);
  });

  it('drops an unrecognised modality rather than passing it on', async () => {
    const models = await read([
      { id: 'm', modalities: ['image', 'hologram', 7, null] },
    ]);
    expect(models[0].modalities).toEqual(['image']);
  });

  it('reads a model with no modality field as text-only', async () => {
    const models = await read([{ id: 'plain' }]);
    expect(models[0].modalities).toEqual([]);
  });

  it('reads a modality list that is not a list as text-only', async () => {
    const models = await read([{ id: 'm', modalities: 'image' }]);
    expect(models[0].modalities).toEqual([]);
  });

  it('reads a thinking declaration without a wire shape', async () => {
    const models = await read([
      {
        id: 'kimi',
        thinking: {
          default: 'max',
          levels: { off: 'false', low: 'low', max: 'max', nonsense: 'x' },
        },
      },
    ]);

    expect(models[0].thinking).toEqual({
      default: 'max',
      levels: { off: 'false', low: 'low', max: 'max' },
    });
  });

  it('drops a recorded wire shape rather than carrying it forward', async () => {
    // An unrecognized key inside a declaration is dropped here, so a save
    // writes the declaration back without it.
    const models = await read([
      {
        id: 'kimi',
        thinking: { default: 'max', levels: { max: 'max' }, wire: 'bailian' },
      },
    ]);

    expect(models[0].thinking).toEqual({
      default: 'max',
      levels: { max: 'max' },
    });
  });

  it('drops a thinking declaration the client cannot use', async () => {
    // A default outside the list would have a session send a level the agent
    // cannot resolve, so the model reads as one with no control instead.
    const models = await read([
      { id: 'm', thinking: { default: 'max', levels: { low: 'low' } } },
      { id: 'n', thinking: { default: 'ultra', levels: { max: 'max' } } },
      { id: 'o', thinking: 'on' },
    ]);

    expect(models.map((model) => model.thinking)).toEqual([
      undefined,
      undefined,
      undefined,
    ]);
  });
});
