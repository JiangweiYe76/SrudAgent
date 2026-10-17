// The form that defines one provider, in a dialog of its own.
//
// Four fields and nothing else. Models are not part of what a provider *is* —
// they are what it comes to offer. They live in their own dialog, opened from
// the provider's row in the panel, where they can be added one at a time.
//
// The panel already has to show the providers that exist, so the fields for one
// of them open in a dialog of their own. It mounts afresh per open, which is
// what resets it — so there is no lingering "editing" state, and cancelling is
// simply not opening it.
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Field, TextInput } from '@/components/ui/field';
import { Modal, ModalContent } from '@/components/ui/modal';
import { PasswordInput } from '@/components/ui/password-input';
import { useProviderStore } from '@/lib/providers';
import type { Provider, ProviderDraft } from '@/lib/acp';
import { t } from '@/lib/i18n';

// What the form holds for a provider that is not there yet.
function blankDraft(): ProviderDraft {
  return { id: '', name: '', baseUrl: '', apiKey: '', models: [] };
}

// The form's starting state for a provider, or for a new one when given none.
//
// `apiKey` is deliberately left off an existing provider: the agent hands back a
// masked key, and sending that back would replace the real credential with its
// own display value. Absent means "keep what is stored".
export function draftOf(provider: Provider | null): ProviderDraft {
  if (!provider) return blankDraft();
  return {
    id: provider.id,
    name: provider.name,
    baseUrl: provider.baseUrl,
    apiKey: undefined,
    models: provider.models,
  };
}

interface ProviderFormModalProps {
  // The provider being changed, or `null` to add one.
  provider: Provider | null;
  onClose: () => void;
}

export function ProviderFormModal({ provider, onClose }: ProviderFormModalProps) {
  const save = useProviderStore((s) => s.save);
  const error = useProviderStore((s) => s.error);

  const [draft, setDraft] = useState(() => draftOf(provider));

  const editing = provider !== null;
  const canSave =
    draft.id.trim() !== '' && draft.name.trim() !== '' && draft.baseUrl.trim() !== '';

  // A refused save leaves the dialog open, so the store's error is readable next
  // to the fields that caused it.
  async function submit() {
    // `draft` carries no models of its own, so the provider's own are handed
    // back untouched: this form is not where they are edited, and a save here
    // must not read as "and delete them all".
    const ok = await save(draft);
    if (ok) onClose();
  }

  return (
    <Modal
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <ModalContent
        title={editing ? t('providers.form.edit', { id: provider.id }) : t('providers.form.add')}
        className="w-[520px]"
        // The settings dialog is already behind this one, so a full scrim would
        // stack with its own.
        overlayClassName="bg-black/20"
      >
        <div className="flex flex-col gap-3 overflow-y-auto p-5">
          {error && (
            <p className="m-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
              {error}
            </p>
          )}

          <div className="flex gap-2">
            <Field label={t('providers.field.id')}>
              <TextInput
                value={draft.id}
                placeholder="my-provider"
                // The id is half of every model reference, so a provider that has
                // one keeps it: renaming would orphan the model of every session
                // left on it.
                disabled={editing}
                autoFocus={!editing}
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

          {editing && (
            <p className="m-0 text-[11px] text-muted-foreground">
              {t('providers.form.models.hint', { n: provider.models.length })}
            </p>
          )}

          <div className="flex justify-end gap-2 pt-1">
            <Button variant="ghost" onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button onClick={() => void submit()} disabled={!canSave}>
              {t('providers.form.save')}
            </Button>
          </div>
        </div>
      </ModalContent>
    </Modal>
  );
}
