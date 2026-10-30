import { useEffect, useRef, useState } from 'react';
import { t } from '@/lib/i18n';
import { cn } from '@/lib/utils';
import {
  DEFAULT_BINDINGS,
  SHORTCUT_ACTIONS,
  bindingFromEvent,
  conflictFor,
  formatBinding,
  parseBinding,
  useShortcutStore,
  type ShortcutId,
} from '@/lib/shortcuts';

/** The keyboard settings: one row per action, each recording its own binding. */
export function ShortcutsPanel() {
  const bindings = useShortcutStore((s) => s.bindings);
  const recording = useShortcutStore((s) => s.recording);
  const beginRecording = useShortcutStore((s) => s.beginRecording);
  const endRecording = useShortcutStore((s) => s.endRecording);
  const setBinding = useShortcutStore((s) => s.setBinding);
  const resetBinding = useShortcutStore((s) => s.resetBinding);
  // The last refusal, kept with the row it belongs to, so it appears under the
  // keys the user is working on rather than at the foot of the list.
  const [error, setError] = useState<{ id: ShortcutId; message: string } | null>(null);
  const listeningRef = useRef<HTMLButtonElement>(null);

  // A button click does not put the button in focus on every platform, and a
  // control that is not focused hears no keys.
  useEffect(() => {
    if (recording) listeningRef.current?.focus();
  }, [recording]);

  const capture = (event: React.KeyboardEvent<HTMLButtonElement>, id: ShortcutId) => {
    // Both halves matter: the dialog would read Escape as "close me", and the
    // window listener would read the new keys as the bindings they already are.
    event.preventDefault();
    event.stopPropagation();
    if (event.key === 'Escape') {
      endRecording();
      setError(null);
      return;
    }
    const binding = bindingFromEvent(event.nativeEvent);
    if (!binding) return;
    if (!parseBinding(binding)) {
      setError({ id, message: t('shortcuts.needModifier') });
      return;
    }
    const taken = conflictFor(bindings, id, binding);
    if (taken) {
      setError({ id, message: t('shortcuts.taken', { name: t(taken.labelKey) }) });
      return;
    }
    setBinding(id, binding);
    endRecording();
    setError(null);
  };

  return (
    <div className="flex flex-col gap-4">
      <p className="m-0 text-xs text-muted-foreground">{t('shortcuts.hint')}</p>
      <div className="flex flex-col">
        {SHORTCUT_ACTIONS.map((action) => {
          const listening = recording === action.id;
          const rowError = error?.id === action.id ? error.message : null;
          const changed = bindings[action.id] !== DEFAULT_BINDINGS[action.id];
          return (
            <div
              key={action.id}
              className="flex flex-col gap-1.5 border-b border-border py-3 first:pt-0 last:border-b-0 last:pb-0"
            >
              <div className="flex items-center justify-between gap-3">
                <span className="text-sm">{t(action.labelKey)}</span>
                <div className="flex shrink-0 items-center gap-2">
                  <button
                    ref={listening ? listeningRef : undefined}
                    onClick={() => {
                      setError(null);
                      if (listening) endRecording();
                      else beginRecording(action.id);
                    }}
                    onKeyDown={(event) => listening && capture(event, action.id)}
                    onBlur={() => {
                      if (listening) endRecording();
                    }}
                    title={t(action.labelKey)}
                    className={cn(
                      'min-w-[92px] cursor-pointer rounded-md border px-2.5 py-1 text-center font-mono text-xs transition-colors',
                      listening
                        ? 'border-accent bg-item-hover text-accent'
                        : 'border-border text-muted-foreground hover:bg-item-hover hover:text-foreground',
                    )}
                  >
                    {listening ? t('shortcuts.recording') : formatBinding(bindings[action.id])}
                  </button>
                  {/* Only where there is something to undo: a default row with a
                      reset beside it reads as a button that would do nothing. */}
                  {changed && !listening && (
                    <button
                      onClick={() => {
                        resetBinding(action.id);
                        setError(null);
                      }}
                      className="cursor-pointer text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                    >
                      {t('shortcuts.reset')}
                    </button>
                  )}
                </div>
              </div>
              {rowError && <span className="text-xs text-destructive">{rowError}</span>}
            </div>
          );
        })}
      </div>
    </div>
  );
}
