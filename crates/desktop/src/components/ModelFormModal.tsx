// One model, in a dialog of its own.
//
// The fields would fit inline, but they are edited from inside the models dialog
// and there is nowhere else for them to live: that dialog is itself a dialog, so
// the third level is simply the next one up the stack. What buys the extra layer
// is that a model is a real row in the provider's list rather than a line of
// text — it can be read back, changed and removed on its own.
import { useState, type ReactNode } from 'react';
import { Image, Music, Video } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Field, TextInput, ToggleGroup } from '@/components/ui/field';
import { Modal, ModalContent } from '@/components/ui/modal';
import { MODALITIES, type Modality, type ProviderModel } from '@/lib/acp';
import { t } from '@/lib/i18n';

// The icon each modality shows, in the same order the protocol lists them.
const MODALITY_ICONS: Record<Modality, ReactNode> = {
  image: <Image className="h-3.5 w-3.5 shrink-0 self-center align-middle" />,
  audio: <Music className="h-3.5 w-3.5" />,
  video: <Video className="h-3.5 w-3.5" />,
};

interface ModelFormModalProps {
  // The model being changed, or `null` to add one.
  model: ProviderModel | null;
  // Ids the provider already uses, so a duplicate is caught while it is being
  // typed rather than by the agent refusing the whole provider.
  taken: string[];
  onSave: (model: ProviderModel) => void;
  onClose: () => void;
}

export function ModelFormModal({ model, taken, onSave, onClose }: ModelFormModalProps) {
  const [id, setId] = useState(() => model?.id ?? '');
  const [name, setName] = useState(() => model?.name ?? '');
  const [modalities, setModalities] = useState<Modality[]>(() => model?.modalities ?? []);
  const [context, setContext] = useState(() =>
    model?.contextWindow === undefined ? '' : String(model.contextWindow),
  );

  const trimmed = id.trim();
  const editing = model !== null;
  // A blank window is "not configured"; anything else has to read as a whole
  // number of tokens above zero, which is what the agent will store.
  const contextTrimmed = context.trim();
  const contextNumber =
    contextTrimmed === '' ? undefined : Number(contextTrimmed.replace(/_/g, ''));
  const contextProblem =
    contextNumber !== undefined &&
    (!Number.isInteger(contextNumber) || contextNumber <= 0)
      ? t('providers.model.badContext')
      : null;
  // The agent's own rules, in the same words it would refuse with: an id cannot
  // be blank, cannot carry `/` (which separates a reference from its provider),
  // and cannot be listed twice under one provider.
  const problem =
    trimmed.includes('/')
      ? t('providers.model.badId')
      : taken.includes(trimmed)
        ? t('providers.model.duplicate', { id: trimmed })
        : contextProblem;

  function save() {
    const label = name.trim();
    const kept = MODALITIES.filter((modality) => modalities.includes(modality));
    onSave({
      id: trimmed,
      ...(label ? { name: label } : {}),
      modalities: kept,
      ...(contextNumber !== undefined ? { contextWindow: contextNumber } : {}),
    });
    onClose();
  }

  return (
    <Modal
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <ModalContent
        title={editing ? t('providers.model.form.edit', { id: model.id }) : t('providers.model.form.add')}
        className="w-[400px]"
        overlayClassName="bg-black/15"
      >
        <div className="flex flex-col gap-3 p-5">
          <Field label={t('providers.model.field.id')}>
            <TextInput value={id} placeholder="my-model" onChange={setId} />
          </Field>

          <Field
            label={t('providers.model.field.name')}
            hint={
              <span className="text-[11px] text-muted-foreground">
                {t('providers.model.field.name.hint')}
              </span>
            }
          >
            <TextInput value={name} placeholder="My model" onChange={setName} />
          </Field>

          <Field
            label={t('providers.model.field.accepts')}
            hint={
              <span className="text-[11px] text-muted-foreground">
                {t('providers.model.field.accepts.hint')}
              </span>
            }
          >
            <ToggleGroup
              options={MODALITIES.map((modality) => ({
                value: modality,
                label: t(`providers.modality.${modality}`),
                icon: MODALITY_ICONS[modality],
              }))}
              value={modalities}
              onChange={setModalities}
            />
          </Field>

          <Field
            label={t('providers.model.field.context')}
            hint={
              <span className="text-[11px] text-muted-foreground">
                {t('providers.model.field.context.hint')}
              </span>
            }
          >
            <TextInput value={context} placeholder="128000" onChange={setContext} />
          </Field>

          {problem && (
            <p className="m-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
              {problem}
            </p>
          )}

          <div className="flex justify-end gap-2 pt-1">
            <Button variant="ghost" onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button onClick={save} disabled={trimmed === '' || problem !== null}>
              {t('providers.form.save')}
            </Button>
          </div>
        </div>
      </ModalContent>
    </Modal>
  );
}
