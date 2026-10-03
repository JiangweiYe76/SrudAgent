import { useState } from 'react';
import { ArrowUp, Plus, Square } from 'lucide-react';
import { t } from '@/lib/i18n';
import { Button } from '@/components/ui/button';

interface ChatInputProps {
  onSend: (text: string) => void;
  onNew: () => void;
  // True while the session has a turn in flight. Stop is always rendered, so
  // the row does not shift when a turn starts; it is only clickable while busy.
  busy: boolean;
  onStop: () => void;
}

export function ChatInput({ onSend, onNew, busy, onStop }: ChatInputProps) {
  const [value, setValue] = useState('');

  const submit = () => {
    const text = value.trim();
    if (!text || busy) return;
    onSend(text);
    setValue('');
  };

  return (
    <div className="absolute inset-x-0 bottom-[18px] z-10 mx-auto flex w-full max-w-[836px] flex-col gap-2 rounded-2xl border border-border bg-background p-[14px_16px_10px] shadow-[0_2px_12px_rgba(0,0,0,0.10)] focus-within:border-accent dark:shadow-[0_2px_12px_rgba(0,0,0,0.55)]">
      <textarea
        className="min-h-[44px] max-h-[200px] resize-none border-none bg-transparent text-sm leading-normal text-foreground outline-none placeholder:text-muted-foreground"
        value={value}
        placeholder={t('chat.placeholder')}
        rows={2}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            submit();
          }
        }}
      />
      <div className="flex items-center justify-between">
        <Button
          variant="ghost"
          size="icon"
          onClick={onNew}
          title={t('sidebar.newChat')}
          aria-label={t('sidebar.newChat')}
        >
          <Plus className="h-4 w-4" />
        </Button>
        <div className="flex items-center gap-1.5">
          <Button
            variant="outline"
            size="icon"
            onClick={onStop}
            disabled={!busy}
            title={t('chat.stop')}
            aria-label={t('chat.stop')}
          >
            <Square className="h-3 w-3 fill-current" />
          </Button>
          <Button
            size="icon"
            onClick={submit}
            disabled={!value.trim() || busy}
            title={t('chat.send')}
            aria-label={t('chat.send')}
            className="disabled:bg-border disabled:text-muted-foreground"
          >
            <ArrowUp className="h-4 w-4" />
          </Button>
        </div>
      </div>
    </div>
  );
}
