// Providers and their models, as one master-detail pane.
//
// The left column is the list; the right is the chosen provider, edited in
// place — its fields, and beneath them its models. Nothing here opens a dialog
// of its own: the settings dialog is the only layer, and the detail pane is
// where every field lives.
//
// The panel is a thin skin over the agent's own methods: every call answers
// with the whole catalog, so it holds a selection and nothing else.
import { useEffect, useState } from 'react';
import { Check, ChevronDown, Plus, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { ProviderDetail } from '@/components/ProviderDetail';
import { defaultChoices, useProviderStore } from '@/lib/providers';
import { cn } from '@/lib/utils';
import { t } from '@/lib/i18n';

export function ProvidersPanel() {
  const providers = useProviderStore((s) => s.providers);
  const defaultModel = useProviderStore((s) => s.defaultModel);
  const loaded = useProviderStore((s) => s.loaded);
  const error = useProviderStore((s) => s.error);
  const load = useProviderStore((s) => s.load);
  const remove = useProviderStore((s) => s.remove);
  const setDefault = useProviderStore((s) => s.setDefault);
  const [busy, setBusy] = useState(false);
  // Which provider the detail pane shows, by id; `null` is "none yet". The
  // pane itself is the only place fields are edited, so this is all the
  // state the list keeps.
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  useEffect(() => {
    void load();
  }, [load]);

  // The selection survives on a provider that exists. The first load selects
  // the first provider, so the pane is never empty while there is something to
  // show; a provider deleted elsewhere empties the pane rather than showing a
  // stale one.
  useEffect(() => {
    if (!loaded) return;
    setSelectedId((current) =>
      current !== null && providers.some((provider) => provider.id === current)
        ? current
        : (providers[0]?.id ?? null),
    );
  }, [loaded, providers]);

  const selected = adding
    ? null
    : (providers.find((provider) => provider.id === selectedId) ?? null);

  const choices = defaultChoices(providers);
  // A stored default can name a model that is not on offer any more — a provider
  // deleted since, or a model removed from one. Then the select shows it as
  // itself rather than silently reading as something else.
  const orphaned =
    defaultModel !== null && !choices.some((c) => c.value === defaultModel);

  async function removeProvider(id: string) {
    setBusy(true);
    await remove(id);
    setBusy(false);
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4">
      {/* First, because it is the one thing here that every new session inherits
          without anyone choosing it again. */}
      <div className="flex flex-col gap-1">
        <span className="text-sm">{t('providers.default')}</span>
        <span className="text-xs text-muted-foreground">
          {t('providers.default.hint')}
        </span>
        <DropdownMenu>
          <DropdownMenuTrigger
            aria-label={t('providers.default')}
            disabled={busy || choices.length === 0}
            className={cn(
              'mt-1 flex w-full cursor-pointer items-center justify-between gap-2 rounded-md border border-border bg-background px-2 py-1.5 text-left text-[13px] outline-none transition-colors',
              'hover:bg-item-hover focus-visible:border-accent disabled:pointer-events-none disabled:opacity-60',
              orphaned
                ? 'text-destructive'
                : defaultModel === null
                  ? 'text-muted-foreground'
                  : undefined,
            )}
          >
            <span className="truncate">
              {defaultModel === null
                ? t('providers.default.unset')
                : orphaned
                  ? t('models.unavailable', { model: defaultModel })
                  : (choices.find((choice) => choice.value === defaultModel)
                      ?.label ?? defaultModel)}
            </span>
            <ChevronDown className="h-3.5 w-3.5 shrink-0" />
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="start"
            sideOffset={4}
            className="max-h-[320px] w-[var(--radix-dropdown-menu-trigger-width)] min-w-[200px] overflow-y-auto"
          >
            {/* The stored default can name a model that is no longer offered;
                listing it explains the trigger's unavailable text. */}
            {orphaned && (
              <DropdownMenuItem disabled className="text-destructive">
                <span className="truncate">
                  {t('models.unavailable', { model: defaultModel })}
                </span>
              </DropdownMenuItem>
            )}
            {choices.map((choice) => {
              // Picking what is already the default would change nothing.
              const current = choice.value === defaultModel;
              return (
                <DropdownMenuItem
                  key={choice.value}
                  disabled={current}
                  onSelect={() => void setDefault(choice.value)}
                >
                  <Check
                    className={cn(
                      'h-3.5 w-3.5 shrink-0',
                      current ? 'opacity-100' : 'opacity-0',
                    )}
                  />
                  <span className="truncate">{choice.label}</span>
                </DropdownMenuItem>
              );
            })}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <div className="flex min-h-0 flex-1 gap-4">
        <div className="flex w-[220px] shrink-0 flex-col gap-1.5">
          <Button
            variant={adding ? 'default' : 'outline'}
            className="shrink-0"
            aria-pressed={adding}
            onClick={() => setAdding(true)}
          >
            <Plus className="h-3.5 w-3.5" />
            {t('providers.add')}
          </Button>

          {error && (
            <p className="m-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
              {error}
            </p>
          )}

          {!loaded ? (
            <span className="text-xs text-muted-foreground">
              {t('providers.loading')}
            </span>
          ) : providers.length === 0 ? (
            <span className="text-xs text-muted-foreground">
              {t('providers.empty')}
            </span>
          ) : (
            <ul className="m-0 flex min-h-0 list-none flex-col gap-1 overflow-y-auto p-0">
              {providers.map((provider) => (
                <li
                  key={provider.id}
                  className={cn(
                    'flex items-center gap-1 rounded-lg border border-border pr-1',
                    !adding &&
                      selectedId === provider.id &&
                      'border-accent bg-item-hover',
                  )}
                >
                  <button
                    type="button"
                    aria-current={!adding && selectedId === provider.id}
                    onClick={() => {
                      setAdding(false);
                      setSelectedId(provider.id);
                    }}
                    className="flex min-w-0 flex-1 cursor-pointer flex-col items-start gap-0.5 rounded-lg px-2.5 py-2 text-left outline-none transition-colors hover:bg-item-hover"
                  >
                    <span className="w-full truncate text-[13px]">
                      {provider.name}
                    </span>
                    <span className="w-full truncate text-[11px] text-muted-foreground">
                      {provider.id} ·{' '}
                      {t('providers.modelCount', { n: provider.models.length })}{' '}
                      ·{' '}
                      {provider.hasKey
                        ? t('providers.key.set', { key: provider.apiKey ?? '' })
                        : t('providers.key.none')}
                    </span>
                  </button>
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

        <div className="flex min-w-0 flex-1 border-l border-border pl-4">
          {adding ? (
            <ProviderDetail
              key="new"
              provider={null}
              onSaved={(id) => {
                setAdding(false);
                setSelectedId(id);
              }}
              onCancel={() => setAdding(false)}
            />
          ) : selected ? (
            <ProviderDetail
              key={selected.id}
              provider={selected}
              // An existing provider has nowhere else to go: a cancel resets
              // the fields in the pane itself, and a save keeps it there.
              onSaved={() => {}}
              onCancel={() => {}}
            />
          ) : (
            !loaded && (
              <span className="text-xs text-muted-foreground">
                {t('providers.loading')}
              </span>
            )
          )}
        </div>
      </div>
    </div>
  );
}
