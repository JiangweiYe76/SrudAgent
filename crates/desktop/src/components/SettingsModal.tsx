import { useState } from 'react';
import { Boxes, Info, Keyboard, Monitor, Moon, Palette, Sun, type LucideIcon } from 'lucide-react';
import { t } from '@/lib/i18n';
import { cn } from '@/lib/utils';
import { Modal, ModalContent } from '@/components/ui/modal';
import { ProvidersPanel } from '@/components/ProvidersPanel';
import { ShortcutsPanel } from '@/components/ShortcutsPanel';
import { useThemeStore, type ThemeMode } from '@/lib/theme';
import { percentOf, useZoomStore, ZOOM_LEVELS } from '@/lib/zoom';

interface SettingsModalProps {
  open: boolean;
  onClose: () => void;
}

type Section = 'appearance' | 'shortcuts' | 'models' | 'about';

const SECTIONS: { id: Section; label: Parameters<typeof t>[0]; Icon: LucideIcon }[] = [
  { id: 'appearance', label: 'settings.appearance', Icon: Palette },
  { id: 'shortcuts', label: 'settings.shortcuts', Icon: Keyboard },
  { id: 'models', label: 'settings.models', Icon: Boxes },
  { id: 'about', label: 'settings.about', Icon: Info },
];

const THEME_OPTIONS: { mode: ThemeMode; label: Parameters<typeof t>[0]; Icon: LucideIcon }[] = [
  { mode: 'system', label: 'settings.theme.system', Icon: Monitor },
  { mode: 'light', label: 'settings.theme.light', Icon: Sun },
  { mode: 'dark', label: 'settings.theme.dark', Icon: Moon },
];

// The heading over each section's own content.
const TITLES: Record<Section, Parameters<typeof t>[0]> = {
  appearance: 'settings.appearance',
  shortcuts: 'settings.shortcuts',
  models: 'settings.models',
  about: 'settings.about',
};

export function SettingsModal({ open, onClose }: SettingsModalProps) {
  const mode = useThemeStore((s) => s.mode);
  const setMode = useThemeStore((s) => s.setMode);
  const level = useZoomStore((s) => s.level);
  const setLevel = useZoomStore((s) => s.setLevel);
  const [section, setSection] = useState<Section>('appearance');

  return (
    <Modal open={open} onOpenChange={(next) => !next && onClose()}>
      <ModalContent title={t('settings.title')} className="h-[640px] w-[960px]">
        <div className="flex min-h-0 flex-1">
          <nav className="flex w-[200px] shrink-0 flex-col gap-0.5 border-r border-border bg-muted p-2">
            {SECTIONS.map(({ id, label, Icon }) => (
              <button
                key={id}
                onClick={() => setSection(id)}
                aria-current={section === id}
                className={cn(
                  'flex cursor-pointer items-center gap-2 rounded-lg px-3 py-2 text-left text-[13px] transition-colors',
                  section === id
                    ? 'bg-item-hover text-foreground'
                    : 'text-muted-foreground hover:bg-item-hover hover:text-foreground',
                )}
              >
                <Icon className="h-4 w-4 shrink-0" />
                {t(label)}
              </button>
            ))}
          </nav>

          {/* Each section scrolls on its own beneath the shared heading. The
              models section does not scroll here at all: its panel is a pair
              of lists that manage their own scrolling. */}
          <div className="flex min-h-0 flex-1 flex-col p-5">
            <span className="shrink-0 text-sm font-semibold">{t(TITLES[section])}</span>

            {section === 'appearance' ? (
              <div className="mt-3 min-h-0 flex-1 overflow-y-auto">
                <div className="flex flex-col gap-6">
                  <div className="flex flex-col gap-3">
                    <div className="flex flex-col gap-1">
                      <span className="text-sm">{t('settings.theme')}</span>
                      <span className="text-xs text-muted-foreground">
                        {t('settings.theme.hint')}
                      </span>
                    </div>

                    <div className="flex gap-2">
                      {THEME_OPTIONS.map(({ mode: option, label, Icon }) => (
                        <button
                          key={option}
                          onClick={() => setMode(option)}
                          aria-pressed={mode === option}
                          className={cn(
                            'flex flex-1 cursor-pointer flex-col items-center gap-1.5 rounded-lg border px-2 py-3 text-[13px] transition-colors',
                            mode === option
                              ? 'border-accent bg-item-hover text-accent'
                              : 'border-border text-muted-foreground hover:bg-item-hover hover:text-foreground',
                          )}
                        >
                          <Icon className="h-4 w-4" />
                          {t(label)}
                        </button>
                      ))}
                    </div>
                  </div>

                  <div className="flex flex-col gap-3">
                    <div className="flex flex-col gap-1">
                      <span className="text-sm">{t('settings.zoom')}</span>
                      <span className="text-xs text-muted-foreground">
                        {t('settings.zoom.hint')}
                      </span>
                    </div>

                    <div className="flex gap-2">
                      {ZOOM_LEVELS.map((option) => (
                        <button
                          key={option}
                          onClick={() => setLevel(option)}
                          aria-pressed={level === option}
                          className={cn(
                            'flex flex-1 cursor-pointer items-center justify-center rounded-lg border px-2 py-3 text-[13px] transition-colors',
                            level === option
                              ? 'border-accent bg-item-hover text-accent'
                              : 'border-border text-muted-foreground hover:bg-item-hover hover:text-foreground',
                          )}
                        >
                          {percentOf(option)}
                        </button>
                      ))}
                    </div>
                  </div>
                </div>
              </div>
            ) : section === 'shortcuts' ? (
              <div className="mt-3 min-h-0 flex-1 overflow-y-auto">
                <ShortcutsPanel />
              </div>
            ) : section === 'models' ? (
              <div className="mt-3 flex min-h-0 flex-1 flex-col">
                <ProvidersPanel />
              </div>
            ) : (
              <div className="mt-3 min-h-0 flex-1 overflow-y-auto">
                <p className="m-0 text-sm text-muted-foreground">{t('settings.about.line')}</p>
              </div>
            )}
          </div>
        </div>
      </ModalContent>
    </Modal>
  );
}
