// The models one provider offers, in a dialog of their own.
//
// Opened from the provider's row in the panel, which is the step that comes
// after adding the provider: a provider is an endpoint and a credential, and a
// model is one of the things that endpoint can be asked for. A provider can be
// added on its own, before any model exists.
//
// Saving writes the whole provider back, because that is what the agent's method
// takes. Everything but the model list travels through unchanged — including the
// key, which is absent from the write and so stays as it is on disk.
import { useState, type ReactNode } from 'react';
import { Image, Layers, Music, Plus, Trash2, Video } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Modal, ModalContent } from '@/components/ui/modal';
import { ModelFormModal } from '@/components/ModelFormModal';
import { useProviderStore } from '@/lib/providers';
import type { Modality, Provider, ProviderModel } from '@/lib/acp';
import { t } from '@/lib/i18n';

// The icon each modality shows wherever a model's inputs are summarised.
const MODALITY_ICONS: Record<Modality, ReactNode> = {
  image: <Image className="h-3 w-3 shrink-0 self-center align-middle" />,
  audio: <Music className="h-3 w-3" />,
  video: <Video className="h-3 w-3" />,
};

interface ModelsModalProps {
  provider: Provider;
  onClose: () => void;
}

export function ModelsModal({ provider, onClose }: ModelsModalProps) {
  const save = useProviderStore((s) => s.save);
  const error = useProviderStore((s) => s.error);

  const [models, setModels] = useState<ProviderModel[]>(() => provider.models);
  // Which model the model dialog is open for: an index into `models`, `null` for
  // one that is not there yet. Whether it is open at all is the outer question.
  const [modelForm, setModelForm] = useState<{ index: number | null } | null>(null);

  // One slot, two verbs: adding appends, changing replaces in place. Doing it
  // here rather than in the dialog keeps the dialog a form and nothing else.
  function putModel(next: ProviderModel) {
    setModels((current) => {
      const index = modelForm?.index ?? null;
      if (index === null) return [...current, next];
      return current.map((model, i) => (i === index ? next : model));
    });
  }

  function dropModel(index: number) {
    setModels((current) => current.filter((_, i) => i !== index));
  }

  async function submit() {
    const ok = await save({
      id: provider.id,
      name: provider.name,
      baseUrl: provider.baseUrl,
      // Absent, not the mask: the key is not being changed here, and sending the
      // display value back would replace the real credential with it.
      models,
    });
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
        title={t('providers.models.title', { name: provider.name })}
        className="w-[480px]"
        // The settings dialog is already behind this one.
        overlayClassName="bg-black/20"
      >
        <div className="flex flex-col gap-3 overflow-y-auto p-5">
          {error && (
            <p className="m-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
              {error}
            </p>
          )}

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

          {models.length === 0 ? (
            <span className="rounded-lg border border-dashed border-border px-3 py-5 text-center text-xs text-muted-foreground">
              {t('providers.model.empty')}
            </span>
          ) : (
            <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
              {models.map((model, index) => (
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

          <div className="flex items-center justify-between gap-2 pt-1">
            <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
              <Layers className="h-3.5 w-3.5" />
              {t('providers.models.of', { id: provider.id })}
            </span>
            <div className="flex gap-2">
              <Button variant="ghost" onClick={onClose}>
                {t('common.cancel')}
              </Button>
              <Button onClick={() => void submit()}>{t('providers.form.save')}</Button>
            </div>
          </div>
        </div>
      </ModalContent>

      {modelForm && (
        <ModelFormModal
          model={modelForm.index === null ? null : (models[modelForm.index] ?? null)}
          // The one being changed is not a duplicate of itself.
          taken={models.filter((_, i) => i !== modelForm.index).map((model) => model.id)}
          onSave={putModel}
          onClose={() => setModelForm(null)}
        />
      )}
    </Modal>
  );
}
