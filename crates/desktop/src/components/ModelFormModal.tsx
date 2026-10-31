// One model, in a dialog of its own.
//
// The fields would fit inline, but they are edited from inside the models dialog
// and there is nowhere else for them to live: that dialog is itself a dialog, so
// the third level is simply the next one up the stack. What buys the extra layer
// is that a model is a real row in the provider's list rather than a line of
// text — it can be read back, changed and removed on its own.
import { useState, type ReactNode } from 'react';
import { Image, Music, Plus, Trash2, Video } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Field, TextInput, ToggleGroup } from '@/components/ui/field';
import { Modal, ModalContent } from '@/components/ui/modal';
import { Select } from '@/components/ui/select';
import { MODALITIES, type Modality, type ProviderModel } from '@/lib/acp';
import { THINKING_LEVELS, thinkingLabel } from '@/lib/providers';
import { t } from '@/lib/i18n';
import type { ThinkingConfig, ThinkingLevel } from '@/lib/types';

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

// One declared level and the value this model is sent for it.
interface ThinkingRow {
  level: ThinkingLevel;
  value: string;
}

// The configured rows of a declaration, in the canonical order. A level left
// out of the config has no row: the editor only shows what the model accepts.
function thinkingRowsOf(config: ThinkingConfig | undefined): ThinkingRow[] {
  if (!config) return [];
  return THINKING_LEVELS.filter((level) => config.levels[level] !== undefined).map((level) => ({
    level,
    value: config.levels[level] ?? '',
  }));
}

export function ModelFormModal({ model, taken, onSave, onClose }: ModelFormModalProps) {
  const [id, setId] = useState(() => model?.id ?? '');
  const [name, setName] = useState(() => model?.name ?? '');
  const [modalities, setModalities] = useState<Modality[]>(() => model?.modalities ?? []);
  const [context, setContext] = useState(() =>
    model?.contextWindow === undefined ? '' : String(model.contextWindow),
  );
  const [thinkingRows, setThinkingRows] = useState<ThinkingRow[]>(() =>
    thinkingRowsOf(model?.thinking),
  );
  const [thinkingDefault, setThinkingDefault] = useState<ThinkingLevel | ''>(
    () => model?.thinking?.default ?? '',
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

  // A declaration is only checked once there is something to declare: an empty
  // list means this model has no thinking control, which is a model saved
  // without a `thinking` field rather than an error. Once a level exists, each
  // one needs a wire value, no level may appear twice, and the default has to
  // name one of them.
  const thinkingProblem =
    thinkingRows.some((row) => row.value.trim() === '')
      ? t('providers.model.thinking.badValue')
      : thinkingRows.some((row, index) =>
            thinkingRows.some((other, i) => i !== index && other.level === row.level),
          )
        ? t('providers.model.thinking.badDuplicate')
        : thinkingRows.length > 0 && !thinkingRows.some((row) => row.level === thinkingDefault)
          ? t('providers.model.thinking.badDefault')
          : null;

  function save() {
    const label = name.trim();
    const kept = MODALITIES.filter((modality) => modalities.includes(modality));
    const thinking: ThinkingConfig | undefined =
      thinkingProblem === null && thinkingRows.length > 0 && thinkingDefault !== ''
        ? {
            default: thinkingDefault,
            levels: thinkingRows.reduce<Partial<Record<ThinkingLevel, string>>>(
              (levels, row) => {
                levels[row.level] = row.value.trim();
                return levels;
              },
              {},
            ),
          }
        : undefined;
    onSave({
      id: trimmed,
      ...(label ? { name: label } : {}),
      modalities: kept,
      ...(contextNumber !== undefined ? { contextWindow: contextNumber } : {}),
      ...(thinking ? { thinking } : {}),
    });
    onClose();
  }

  // Rows offer only levels no other row holds, so a duplicate cannot be made
  // by picking from a menu; the validation above still covers a stored one.
  function levelOptions(row: ThinkingRow) {
    return THINKING_LEVELS.filter(
      (level) =>
        level === row.level || !thinkingRows.some((other) => other.level === level),
    ).map((level) => ({ value: level, label: thinkingLabel(level) }));
  }

  function addThinkingRow() {
    const level = THINKING_LEVELS.find(
      (candidate) => !thinkingRows.some((row) => row.level === candidate),
    );
    if (!level) return;
    setThinkingRows((rows) => [...rows, { level, value: '' }]);
    // The first row also becomes the default: a declaration has to name one,
    // and the level the user just added is the one they are thinking about.
    if (!thinkingRows.some((row) => row.level === thinkingDefault)) {
      setThinkingDefault(level);
    }
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
        className="max-h-[85vh] w-[420px]"
        overlayClassName="bg-black/15"
      >
        <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-5">
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
            as="div"
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

          <div className="flex flex-col gap-2 border-t border-border pt-3">
            <span className="text-xs text-muted-foreground">
              {t('providers.model.field.thinking')}
            </span>

            <Field as="div" label={t('providers.model.thinking.levels')}>
              {thinkingRows.length > 0 && (
                <div className="flex flex-col gap-1.5">
                  {thinkingRows.map((row, index) => (
                    <div key={index} className="flex items-center gap-1.5">
                      <Select
                        ariaLabel={t('providers.model.thinking.level')}
                        className="w-[104px] shrink-0"
                        value={row.level}
                        onChange={(level) =>
                          setThinkingRows((rows) =>
                            rows.map((entry, i) =>
                              i === index ? { ...entry, level: level as ThinkingLevel } : entry,
                            ),
                          )
                        }
                        options={levelOptions(row)}
                      />
                      <TextInput
                        value={row.value}
                        placeholder={row.level}
                        onChange={(value) =>
                          setThinkingRows((rows) =>
                            rows.map((entry, i) => (i === index ? { ...entry, value } : entry)),
                          )
                        }
                      />
                      <Button
                        variant="ghost"
                        size="icon"
                        title={t('providers.model.thinking.remove')}
                        aria-label={t('providers.model.thinking.remove')}
                        onClick={() =>
                          setThinkingRows((rows) => rows.filter((_, i) => i !== index))
                        }
                      >
                        <Trash2 className="h-3.5 w-3.5 text-destructive" />
                      </Button>
                    </div>
                  ))}
                </div>
              )}
              <Button
                type="button"
                variant="outline"
                className="h-7 self-start px-2.5 text-xs"
                onClick={addThinkingRow}
                disabled={thinkingRows.length >= THINKING_LEVELS.length}
              >
                <Plus className="h-3.5 w-3.5" />
                {t('providers.model.thinking.add')}
              </Button>
            </Field>

            {thinkingRows.length > 0 && (
              <Field label={t('providers.model.thinking.default')}>
                <Select
                  ariaLabel={t('providers.model.thinking.default')}
                  value={
                    thinkingRows.some((row) => row.level === thinkingDefault)
                      ? thinkingDefault
                      : ''
                  }
                  onChange={(value) => setThinkingDefault(value as ThinkingLevel | '')}
                  placeholder={t('providers.model.thinking.choose')}
                  options={thinkingRows.map((row) => ({
                    value: row.level,
                    label: thinkingLabel(row.level),
                  }))}
                />
              </Field>
            )}
          </div>

          {(problem || thinkingProblem) && (
            <div className="flex flex-col gap-1.5">
              {problem && (
                <p className="m-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
                  {problem}
                </p>
              )}
              {thinkingProblem && (
                <p className="m-0 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
                  {thinkingProblem}
                </p>
              )}
            </div>
          )}

          <div className="flex justify-end gap-2 pt-1">
            <Button variant="ghost" onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button
              onClick={save}
              disabled={trimmed === '' || problem !== null || thinkingProblem !== null}
            >
              {t('providers.form.save')}
            </Button>
          </div>
        </div>
      </ModalContent>
    </Modal>
  );
}
