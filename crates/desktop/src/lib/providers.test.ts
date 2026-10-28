import { describe, it, expect } from 'vitest';
import { draftModelOptions } from './providers';
import type { Provider } from './acp';

function provider(
  id: string,
  models: Array<{ id: string; name?: string }>,
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

    expect(options?.groups).toEqual([
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
    ]);
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
