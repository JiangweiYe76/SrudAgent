import { describe, it, expect } from 'vitest';
import { draftModelOptions, draftThinkingOptions } from './providers';
import type { Provider } from './acp';
import type { ThinkingConfig } from './types';

function provider(
  id: string,
  models: Array<{ id: string; name?: string; thinking?: ThinkingConfig }>,
): Provider {
  return {
    id,
    name: `${id} name`,
    baseUrl: `https://${id}.example`,
    hasKey: true,
    models: models.map((model) => ({ ...model, modalities: [] })),
  };
}

const PROVIDERS = [
  provider('openai', [
    { id: 'gpt-4o', name: 'GPT-4o' },
    { id: 'o3' },
  ]),
  provider('deepseek', [{ id: 'chat', name: 'Chat' }]),
];

describe('draftModelOptions', () => {
  it('offers every configured model under its provider', () => {
    const options = draftModelOptions(PROVIDERS, null);

    expect(options).toEqual({
      configId: 'model',
      currentValue: '',
      groups: [
        {
          id: 'openai',
          name: 'openai name',
          choices: [
            { value: 'openai/gpt-4o', name: 'GPT-4o' },
            // A model with no display name is shown by its id.
            { value: 'openai/o3', name: 'o3' },
          ],
        },
        {
          id: 'deepseek',
          name: 'deepseek name',
          choices: [{ value: 'deepseek/chat', name: 'Chat' }],
        },
      ],
    });
  });

  it('starts on the configured default', () => {
    expect(draftModelOptions(PROVIDERS, 'deepseek/chat')?.currentValue).toBe('deepseek/chat');
  });

  it('asks for a choice when nothing has been chosen', () => {
    expect(draftModelOptions(PROVIDERS, null)?.currentValue).toBe('');
  });

  it('ignores a default that names nothing', () => {
    // The agent starts a session from a default only when it still resolves, so
    // the draft must not claim a model the session will not run on.
    expect(draftModelOptions(PROVIDERS, 'gone/model')?.currentValue).toBe('');
  });

  it('offers no selector when no provider has a model', () => {
    expect(draftModelOptions([], null)).toBeNull();
    expect(draftModelOptions([provider('empty', [])], null)).toBeNull();
  });
});

describe('draftThinkingOptions', () => {
  const DECLARED = provider('openai', [
    {
      id: 'o3',
      name: 'o3',
      thinking: {
        default: 'medium',
        levels: { off: 'none', medium: 'medium', max: 'high' },
      },
    },
  ]);

  it('offers the declared levels in canonical order', () => {
    // The canonical order, not the order the declaration happens to be written
    // in: a menu that reads weakest to strongest is the one users expect.
    expect(draftThinkingOptions([DECLARED], 'openai/o3', null)).toEqual({
      configId: 'reasoning',
      currentValue: 'medium',
      choices: [
        { value: 'off', name: 'Off' },
        { value: 'medium', name: 'Medium' },
        { value: 'max', name: 'Max' },
      ],
    });
  });

  it('keeps the level the draft already chose', () => {
    expect(draftThinkingOptions([DECLARED], 'openai/o3', 'max')?.currentValue).toBe('max');
  });

  it('falls back to the model default for a level it does not declare', () => {
    // A level the new model does not declare is replaced by that model's
    // default, so the draft never shows a level it cannot send.
    expect(draftThinkingOptions([DECLARED], 'openai/o3', 'low')?.currentValue).toBe('medium');
  });

  it('offers no selector for a model that declares no thinking', () => {
    expect(draftThinkingOptions([], 'openai/o3', null)).toBeNull();
    expect(draftThinkingOptions([provider('openai', [{ id: 'o3' }])], 'openai/o3', null)).toBeNull();
    // A reference that names nothing has nothing to declare.
    expect(draftThinkingOptions([DECLARED], '', null)).toBeNull();
    expect(draftThinkingOptions([DECLARED], 'openai', null)).toBeNull();
  });
});
