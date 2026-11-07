// The configured providers, as the settings panel sees them.
//
// Unlike the theme and zoom stores, whose truth is `localStorage` because it is
// a property of this machine's UI, the truth here is a file the agent owns. So
// every read and every write goes through the agent, and this store is only the
// last catalog it answered with.
import { create } from 'zustand';
import {
  deleteProvider as requestDelete,
  listProviders,
  MODEL_OPTION_ID,
  RpcFailure,
  saveProvider as requestSave,
  setDefaultModel as requestDefault,
  THINKING_OPTION_ID,
  type Provider,
  type ProviderCatalog,
  type ProviderDraft,
} from './acp';
import { t, type Key } from './i18n';
import {
  THINKING_LEVELS,
  type ModelOptions,
  type ThinkingLevel,
  type ThinkingOptions,
} from './types';

// The canonical levels are declared with the domain types, and re-exported here
// so the settings panel has one import for everything a model declares.
export { THINKING_LEVELS };

interface ProviderState {
  providers: Provider[];
  defaultModel: string | null;
  // False until a listing has come back, so the panel can tell "nothing is
  // configured" apart from "not asked yet".
  loaded: boolean;
  // The last refusal, kept rather than rethrown: the panel is where it belongs,
  // and a failure to list is not a reason to take the window down.
  error: ProviderRefusal | null;
  load: () => Promise<void>;
  save: (draft: ProviderDraft) => Promise<boolean>;
  remove: (id: string) => Promise<boolean>;
  setDefault: (model: string) => Promise<boolean>;
}

/**
 * A change the agent refused, in the shape its wording needs.
 *
 * Keeping it structured rather than pre-formatted is what lets the panel say it
 * in the reader's language: `kind` names the rule, `params` are the values the
 * sentence places, and `detail` is the agent's own words for the rules this
 * client has no wording for — or for a failure that never reached the agent.
 */
export interface ProviderRefusal {
  /** What the agent said, shown when the rule has no wording here. */
  detail: string;
  /** The rule the agent named, when it names one. */
  kind: string | null;
  /** The values that rule's wording places. */
  params: Record<string, string>;
}

// The wording each rule has here. One this table has never heard of is printed
// as the agent wrote it, so a client written before a rule still says something
// true rather than nothing.
const REFUSAL_KEYS: Record<string, Key> = {
  'invalid-id': 'providers.error.invalidId',
  'blank-name': 'providers.error.blankName',
  'bad-base-url': 'providers.error.badBaseUrl',
  'blank-model-id': 'providers.error.blankModelId',
  'duplicate-model': 'providers.model.duplicate',
  'duplicate-provider': 'providers.error.duplicateProvider',
  'thinking-levels-missing': 'providers.error.thinkingLevelsMissing',
  'thinking-default-missing': 'providers.error.thinkingDefaultMissing',
  'thinking-blank-value': 'providers.error.thinkingBlankValue',
  'unknown-provider': 'providers.error.unknownProvider',
  'unknown-model': 'providers.error.unknownModel',
  'bad-reference': 'providers.error.badReference',
};

/** What to show for a refusal, in this build's words. */
export function refusalText(refusal: ProviderRefusal): string {
  const key = refusal.kind === null ? undefined : REFUSAL_KEYS[refusal.kind];
  return key === undefined ? refusal.detail : t(key, refusal.params);
}

function refusalOf(err: unknown): ProviderRefusal {
  if (!(err instanceof RpcFailure)) {
    // A transport failure: no reply, so nothing to word ourselves.
    return { detail: String(err), kind: null, params: {} };
  }
  const data = (err.data ?? {}) as { kind?: unknown; params?: unknown };
  return {
    detail: err.detail,
    kind: typeof data.kind === 'string' ? data.kind : null,
    params: isParams(data.params) ? data.params : {},
  };
}

function isParams(value: unknown): value is Record<string, string> {
  return (
    typeof value === 'object' &&
    value !== null &&
    Object.values(value).every((entry) => typeof entry === 'string')
  );
}

export const useProviderStore = create<ProviderState>((set) => {
  // Every call answers with the whole catalog, so one handler for all of them:
  // a provider added, changed or removed all leave the store holding what the
  // agent says the file says.
  function apply(catalog: ProviderCatalog): boolean {
    set({
      providers: catalog.providers,
      defaultModel: catalog.default,
      error: null,
    });
    return true;
  }

  function failed(err: unknown): boolean {
    set({ error: refusalOf(err) });
    return false;
  }

  return {
    providers: [],
    defaultModel: null,
    loaded: false,
    error: null,

    load: async () => {
      try {
        apply(await listProviders());
      } catch (err) {
        failed(err);
      } finally {
        // Set either way: a listing that failed still answered the question of
        // whether one had been attempted, and leaving it false would keep the
        // panel in its loading state forever.
        set({ loaded: true });
      }
    },

    save: async (draft) => {
      try {
        return apply(await requestSave(draft));
      } catch (err) {
        return failed(err);
      }
    },

    remove: async (id) => {
      try {
        return apply(await requestDelete(id));
      } catch (err) {
        return failed(err);
      }
    },

    setDefault: async (model) => {
      try {
        return apply(await requestDefault(model));
      } catch (err) {
        return failed(err);
      }
    },
  };
});

// Every model reference any configured provider offers, for a default picker
// that has to name one. `providerId/modelId` is the only spelling the agent
// accepts, and is what the panel's option values are.
export interface DefaultChoice {
  value: string;
  label: string;
}

export function defaultChoices(providers: Provider[]): DefaultChoice[] {
  return providers.flatMap((provider) =>
    provider.models.map((model) => ({
      value: `${provider.id}/${model.id}`,
      label: `${provider.name} · ${model.name ?? model.id}`,
    })),
  );
}

// The model selector a new session is offered before the agent has made a
// session for it.
//
// It is the catalog the agent builds a session's selector from, so a draft can
// show what there is to choose and what it will start on without waiting for
// `session/new`. `current` is the model the draft will start on — the configured
// default, or a choice already made — and an empty value means nothing has been
// chosen, the same word the agent sends for a session with no model. A default
// that names nothing is not shown: the agent seeds a session only from a
// reference that still resolves, so the draft would otherwise change its mind
// once the session existed.
//
// The selector is the client's own until the session exists, so it borrows
// SrudAgent's option id; a switch goes through the id the agent returns with
// `session/new`, never through this one.
export function draftModelOptions(
  providers: Provider[],
  current: string | null,
): ModelOptions | null {
  const groups = providers
    .filter((provider) => provider.models.length > 0)
    .map((provider) => ({
      id: provider.id,
      name: provider.name,
      choices: provider.models.map((model) => ({
        value: `${provider.id}/${model.id}`,
        name: model.name ?? model.id,
      })),
    }));
  // No provider offers a model, so there is nothing to pick from and no
  // selector to offer.
  if (groups.length === 0) return null;

  const known =
    current !== null &&
    groups.some((group) =>
      group.choices.some((choice) => choice.value === current),
    );
  return {
    configId: MODEL_OPTION_ID,
    currentValue: known ? current : '',
    groups,
  };
}

// How a level reads in a menu. The wire value is the model's own and is never
// shown.
export function thinkingLabel(level: ThinkingLevel): string {
  switch (level) {
    case 'off':
      return 'Off';
    case 'minimal':
      return 'Minimal';
    case 'low':
      return 'Low';
    case 'medium':
      return 'Medium';
    case 'high':
      return 'High';
    case 'xhigh':
      return 'XHigh';
    case 'max':
      return 'Max';
  }
}

// The thinking selector a draft is offered before the agent has made a session
// for it. Built from the same catalog the agent reads, so the choice made here
// is the one the session starts on, exactly like the draft model selector.
//
// `current` is the level the draft already chose. A level the new model does not
// declare is replaced by that model's default.
export function draftThinkingOptions(
  providers: Provider[],
  modelRef: string,
  current: string | null,
): ThinkingOptions | null {
  // Split on the first slash: provider ids never carry one, model ids may.
  const slash = modelRef.indexOf('/');
  if (slash <= 0) return null;
  const providerId = modelRef.slice(0, slash);
  const modelId = modelRef.slice(slash + 1);
  if (modelId === '') return null;
  const model = providers
    .find((provider) => provider.id === providerId)
    ?.models.find((entry) => entry.id === modelId);
  const config = model?.thinking;
  if (!config) return null;

  const levels = THINKING_LEVELS.filter(
    (level) => config.levels[level] !== undefined,
  );
  if (levels.length === 0) return null;
  const chosen =
    current !== null && levels.includes(current as ThinkingLevel)
      ? (current as ThinkingLevel)
      : config.default;
  return {
    configId: THINKING_OPTION_ID,
    currentValue: chosen,
    choices: levels.map((level) => ({
      value: level,
      name: thinkingLabel(level),
    })),
  };
}
