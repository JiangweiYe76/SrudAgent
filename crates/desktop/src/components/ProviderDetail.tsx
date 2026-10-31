// The detail pane of the providers panel: one provider, edited in place.
//
// The pane holds the provider's own fields and, beneath them, the models it
// offers — one draft of the whole provider, one Save. Adding or changing a
// model swaps the pane to the model form and back; nothing here opens a dialog,
// because the settings dialog is the only layer there is.
//
// The draft starts from what the agent reported. `apiKey` is deliberately left
// off an existing provider: the agent hands back a masked key, and sending that
// back would replace the real credential with its own display value. Absent
// means "keep what is stored".
//
// Each view scrolls on its own, between a fixed header and a fixed save: the
// model list must not carry the save out of view, and the model form's way
// back must not depend on where the scroll has got to.
import { useState } from 'react';
import { ArrowLeft, Image, Layers, Music, Plus, Trash2, Video } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Field, TextInput } from '@/components/ui/field';
import { PasswordInput } from '@/components/ui/password-input';
import { ModelForm } from '@/components/ModelForm';
import { useProviderStore } from '@/lib/providers';
import type { Provider, ProviderDraft, ProviderModel } from '@/lib/acp';
import { t } from '@/lib/i18n';
import type { ReactNode } from 'react';

// The icons each modality shows wherever a model's inputs are summarised.
const MODALITY_ICONS: Record<string, ReactNode> = {
  image: <Image className="h-3 w-3 shrink-0 self-center align-middle" />,
  audio: <Music className="h-3 w-3" />,
  video: <Video className="h-3 w-3" />,
};

// The pane's starting state for a provider, or for a new one when given none.
export function draftOf(provider: Provider | null): ProviderDraft {
  if (!provider) return { id: '', name: '', baseUrl: '', apiKey: '', models: [] };
  return {
    id: provider.id,
    name: provider.name,
    baseUrl: provider.baseUrl,
    apiKey: undefined,
    models: provider.models,
  };
}

interface ProviderDetailProps {
  // The provider being changed, or `null` to add one.
  provider: Provider | null;
  // A save landed. Carries the id as written, so the panel can select a
  // provider that did not exist when the pane was opened.
  onSaved: (id: string) => void;
  // Throw the pane away: an add that is not wanted, from the panel's side.
  onCancel: () => void;
}

export function ProviderDetail({ provider, onSaved, onCancel }: ProviderDetailProps) {
  const save = useProviderStore((s) => s.save);
  const error = useProviderStore((s) => s.error);

  const [draft, setDraft] = useState(() => draftOf(provider));
  // The model form replaces the pane's content while it is open: an index into
  // the draft's models, or `null` for one that is not there yet.
  const [modelForm, setModelForm] = useState<{ index: number | null } | null>(null);

  const editing = provider !== null;
  const canSave =
    draft.id.trim() !== '' && draft.name.trim() !== '' && draft.baseUrl.trim() !== '';

  // One slot, two verbs: adding appends, changing replaces in place. Doing it
  // here rather than in the form keeps the form a form and nothing else.
  function putModel(next: ProviderModel) {
    setDraft((current) => {
      const index = modelForm?.index ?? null;
      if (index === null) return { ...current, models: [...current.models, next] };
      return {
        ...current,
        models: current.models.map((model, i) => (i === index ? next : model)),
      };
    });
  }

  function dropModel(index: number) {
    setDraft((current) => ({
      ...current,
      models: current.models.filter((_, i) => i !== index),
    }));
  }

  // A refused save leaves the pane as it is, so the store's error is readable
  // next to the fields that caused it.
  async function submit() {
    if (await save(draft)) onSaved(draft.id.trim());
  }

  // Cancelling an edit puts the fields back the way the agent reported them;
  // cancelling an add is giving up on the pane, which is the panel's call.
  function cancel() {
    if (editing) setDraft(draftOf(provider));
    else onCancel();
  }

  if (modelForm) {
    const current = modelForm.index === null ? null : (draft.models[modelForm.index] ?? null);
    return (
      <div className="flex min-h-0 flex-1 flex-col gap-3">
        <div className="flex shrink-0 items-center gap-1.5">
          <Button
            variant="ghost"
            size="icon"
            title={t('common.back')}
            aria-label={t('common.back')}
            onClick={() => setModelForm(null)}
          >
            <ArrowLeft className="h-4 w-4" />
          </Button>
          <span className="text-sm font-semibold">
            {current
              ? t('providers.model.form.edit', { id: current.id })
              : t('providers.model.form.add')}
          </span>
        </div>
        <ModelForm
          model={current}
          // The one being changed is not a duplicate of itself.
          taken={draft.models
            .filter((_, i) => i !== modelForm.index)
            .map((model) => model.id)}
          onSave={(model) => {
            putModel(model);
            setModelForm(null);
          }}
          onCancel={() => setModelForm(null)}
        />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      {error && (
        <p className="m-0 shrink-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {error}
        </p>
      )}

      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">
        <div className="flex gap-2">
          <Field label={t('providers.field.id')}>
            <TextInput
              value={draft.id}
              placeholder="my-provider"
              // The id is half of every model reference, so a provider that has
              // one keeps it: renaming would orphan the model of every session
              // left on it.
              disabled={editing}
              onChange={(id) => setDraft((d) => ({ ...d, id }))}
            />
          </Field>
          <Field label={t('providers.field.name')}>
            <TextInput
              value={draft.name}
              placeholder="My provider"
              onChange={(name) => setDraft((d) => ({ ...d, name }))}
            />
          </Field>
        </div>

        <Field label={t('providers.field.baseUrl')}>
          <TextInput
            value={draft.baseUrl}
            placeholder="https://api.example.com/v1"
            onChange={(baseUrl) => setDraft((d) => ({ ...d, baseUrl }))}
          />
        </Field>

        <Field
          label={t('providers.field.apiKey')}
          hint={
            <span className="text-[11px] text-muted-foreground">
              {editing ? t('providers.field.apiKey.keep') : t('providers.field.apiKey.new')}
            </span>
          }
        >
          <PasswordInput
            value={draft.apiKey ?? ''}
            // The stored key, greyed, where it would be if it were being
            // typed — enough to recognise, and never a value the field
            // submits. Leaving it alone keeps it.
            placeholder={editing ? (provider.apiKey ?? '') : 'sk-…'}
            onChange={(apiKey) => setDraft((d) => ({ ...d, apiKey }))}
          />
        </Field>

        {/* Models are declared once the provider exists: a provider is added
            before it has any, and the list below is where they gather. */}
        {editing && (
          <div className="flex flex-col gap-2 border-t border-border pt-3">
            <div className="flex items-start justify-between gap-3">
              <span className="text-xs text-muted-foreground">
                {t('providers.models.hint')}
              </span>
              <Button
                variant="outline"
                className="shrink-0"
                onClick={() => setModelForm({ index: null })}
              >
                <Plus className="h-3.5 w-3.5" />
                {t('providers.model.add')}
              </Button>
            </div>

            {draft.models.length === 0 ? (
              <span className="rounded-lg border border-dashed border-border px-3 py-4 text-center text-xs text-muted-foreground">
                {t('providers.model.empty')}
              </span>
            ) : (
              <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
                {draft.models.map((model, index) => (
                  <li
                    key={`${model.id}-${index}`}
                    className="flex items-center gap-2 rounded-lg border border-border px-2 py-1.5"
                  >
                    <button
                      type="button"
                      title={t('providers.model.edit')}
                      onClick={() => setModelForm({ index })}
                      className="flex min-w-0 flex-1 cursor-pointer flex-col items-start text-left"
                    >
                      <span className="flex w-full items-center gap-2">
                        <span className="min-w-0 flex-1 truncate text-[13px]">
                          {model.name ?? model.id}
                        </span>
                        {/* What it takes besides text, as icons: a row is a
                            summary, and three words per model crowd it. */}
                        {model.modalities.length > 0 && (
                          <span className="flex shrink-0 items-center gap-1 text-muted-foreground">
                            {model.modalities.map((modality) => (
                              <span key={modality} title={t(`providers.modality.${modality}`)}>
                                {MODALITY_ICONS[modality]}
                              </span>
                            ))}
                          </span>
                        )}
                      </span>
                      {/* The id is what a reference is built from, so it is always
                          on screen — not only when it differs from the label
                          someone typed on top of it. */}
                      {model.name && (
                        <span className="w-full truncate font-mono text-[11px] text-muted-foreground">
                          {model.id}
                        </span>
                      )}
                    </button>
                    <Button
                      variant="ghost"
                      size="icon"
                      title={t('providers.model.remove')}
                      aria-label={t('providers.model.remove')}
                      onClick={() => dropModel(index)}
                    >
                      <Trash2 className="h-3.5 w-3.5 text-destructive" />
                    </Button>
                  </li>
                ))}
              </ul>
            )}

            <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
              <Layers className="h-3.5 w-3.5" />
              {t('providers.models.of', { id: draft.id })}
            </span>
          </div>
        )}
      </div>

      <div className="flex shrink-0 justify-end gap-2">
        <Button variant="ghost" onClick={cancel}>
          {t('common.cancel')}
        </Button>
        <Button onClick={() => void submit()} disabled={!canSave}>
          {t('providers.form.save')}
        </Button>
      </div>
    </div>
  );
}
