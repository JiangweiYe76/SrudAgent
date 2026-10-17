import { useEffect, useState } from 'react';
import { Layers, Pencil, Plus, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { defaultChoices, useProviderStore } from '@/lib/providers';
import type { Provider } from '@/lib/acp';
import { cn } from '@/lib/utils';
import { t } from '@/lib/i18n';

// Providers, their credentials and their models.
//
// The panel is a thin skin over the agent's own methods: every call answers with
// the whole catalog, so it holds a list and nothing else. Changing a provider
// happens in the provider dialog and its models in the models dialog, both of
// which the panel only decides to open — and which it mounts only while open, so
// a visit never inherits the last one's fields.
interface ProvidersPanelProps {
  form: { provider: Provider | null } | null;
  onFormChange: (form: { provider: Provider | null } | null) => void;
  modelsFor: Provider | null;
  onModelsForChange: (provider: Provider | null) => void;
}

export function ProvidersPanel({ form, onFormChange, modelsFor, onModelsForChange }: ProvidersPanelProps) {
  const providers = useProviderStore((s) => s.providers);
  const defaultModel = useProviderStore((s) => s.defaultModel);
  const loaded = useProviderStore((s) => s.loaded);
  const error = useProviderStore((s) => s.error);
  const load = useProviderStore((s) => s.load);
  const remove = useProviderStore((s) => s.remove);
  const setDefault = useProviderStore((s) => s.setDefault);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void load();
  }, [load]);

  const choices = defaultChoices(providers);
  // A stored default can name a model that is not on offer any more — a provider
  // deleted since, or a model removed from one. Then the select shows it as
  // itself rather than silently reading as something else.
  const orphaned = defaultModel !== null && !choices.some((c) => c.value === defaultModel);

  async function removeProvider(id: string) {
    setBusy(true);
    await remove(id);
    setBusy(false);
  }

  return (
    <div className="flex flex-col gap-6">
      {/* First, because it is the one thing here that every new session inherits
          without anyone choosing it again. */}
      <label className="flex flex-col gap-1">
        <span className="text-sm">{t('providers.default')}</span>
        <span className="text-xs text-muted-foreground">{t('providers.default.hint')}</span>
        <select
          value={defaultModel ?? ''}
          disabled={busy || choices.length === 0}
          onChange={(event) => {
            const value = event.target.value;
            if (value) void setDefault(value);
          }}
          className="mt-1 rounded-md border border-border bg-background px-2 py-1.5 text-[13px] outline-none focus-visible:border-accent disabled:opacity-60"
        >
          {/* Nobody has chosen yet, or what they chose has gone. Either way the
              select says so rather than standing in for a model that is not
              theirs to pick. Disabled, so it cannot be re-chosen by accident. */}
          {defaultModel === null && (
            <option value="" disabled>
              {t('providers.default.unset')}
            </option>
          )}
          {orphaned && (
            <option value={defaultModel} disabled>
              {t('models.unavailable', { model: defaultModel })}
            </option>
          )}
          {choices.map((choice) => (
            <option key={choice.value} value={choice.value}>
              {choice.label}
            </option>
          ))}
        </select>
      </label>

      <div className="flex flex-col gap-3">
        <div className="flex items-start justify-between gap-4">
          <div className="flex min-w-0 flex-col gap-1">
            <span className="text-sm">{t('providers.title')}</span>
            <span className="text-xs text-muted-foreground">{t('providers.hint')}</span>
          </div>
          <Button onClick={() => onFormChange({ provider: null })} className="shrink-0">
            <Plus className="h-3.5 w-3.5" />
            {t('providers.add')}
          </Button>
        </div>

        {error && !form && !modelsFor && (
          <p className="m-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
            {error}
          </p>
        )}

        {!loaded ? (
          <span className="text-xs text-muted-foreground">{t('providers.loading')}</span>
        ) : providers.length === 0 ? (
          <span className="text-xs text-muted-foreground">{t('providers.empty')}</span>
        ) : (
          <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
            {providers.map((provider) => (
              <li
                key={provider.id}
                className={cn(
                  'flex items-center gap-3 rounded-lg border border-border px-3 py-2',
                  form?.provider?.id === provider.id && 'border-accent',
                )}
              >
                <div className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-[13px]">{provider.name}</span>
                  <span className="truncate text-[11px] text-muted-foreground">
                    {provider.id} · {provider.baseUrl} ·{' '}
                    {provider.hasKey
                      ? t('providers.key.set', { key: provider.apiKey ?? '' })
                      : t('providers.key.none')}
                  </span>
                </div>
                {/* Its own dialog, because a provider is added before it has any
                    model and the two are edited at different times. */}
                <Button
                  variant="outline"
                  className="h-7 shrink-0 px-2 text-xs"
                  title={t('providers.models.manage')}
                  aria-label={t('providers.models.manage')}
                  onClick={() => onModelsForChange(provider)}
                >
                  <Layers className="h-3.5 w-3.5" />
                  {t('providers.modelCount', { n: provider.models.length })}
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  title={t('providers.edit')}
                  aria-label={t('providers.edit')}
                  onClick={() => onFormChange({ provider })}
                >
                  <Pencil className="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  title={t('providers.delete')}
                  aria-label={t('providers.delete')}
                  disabled={busy}
                  onClick={() => void removeProvider(provider.id)}
                >
                  <Trash2 className="h-3.5 w-3.5 text-destructive" />
                </Button>
              </li>
            ))}
          </ul>
        )}
      </div>

    </div>
  );
}
