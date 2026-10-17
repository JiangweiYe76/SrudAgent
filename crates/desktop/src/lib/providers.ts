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
  saveProvider as requestSave,
  setDefaultModel as requestDefault,
  type Provider,
  type ProviderCatalog,
  type ProviderDraft,
} from './acp';

interface ProviderState {
  providers: Provider[];
  defaultModel: string | null;
  // False until a listing has come back, so the panel can tell "nothing is
  // configured" apart from "not asked yet".
  loaded: boolean;
  // The last refusal, kept rather than rethrown: the panel is where it belongs,
  // and a failure to list is not a reason to take the window down.
  error: string | null;
  load: () => Promise<void>;
  save: (draft: ProviderDraft) => Promise<boolean>;
  remove: (id: string) => Promise<boolean>;
  setDefault: (model: string) => Promise<boolean>;
}

export const useProviderStore = create<ProviderState>((set) => {
  // Every call answers with the whole catalog, so one handler for all of them:
  // a provider added, changed or removed all leave the store holding what the
  // agent says the file says.
  function apply(catalog: ProviderCatalog): boolean {
    set({ providers: catalog.providers, defaultModel: catalog.default, error: null });
    return true;
  }

  function failed(err: unknown): boolean {
    set({ error: String(err) });
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
