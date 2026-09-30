import { useState } from 'react';
import { ArrowUp, Plus } from 'lucide-react';
import { t } from '@/lib/i18n';
import { Button } from '@/components/ui/button';

interface ChatInputProps {
  onSend: (text: string) => void;
  onNew: () => void;
}

export function ChatInput({ onSend, onNew }: ChatInputProps) {
  const [value, setValue] = useState('');

  const submit = () => {
    const text = value.trim();
    if (!text) return;
    onSend(text);
    setValue('');
  };

  return (
    <div className="absolute inset-x-0 bottom-[18px] z-10 mx-auto flex w-full max-w-[836px] flex-col gap-2 rounded-2xl border border-border bg-background p-[14px_16px_10px] shadow-[0_2px_12px_rgba(0,0,0,0.10)] focus-within:border-accent">
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
        <Button variant="ghost" size="icon" onClick={onNew} title={t('sidebar.newChat')}>
          <Plus className="h-4 w-4" />
        </Button>
        <Button
          size="icon"
          onClick={submit}
          disabled={!value.trim()}
          title={t('chat.send')}
          className="h-8 w-8 rounded-full disabled:bg-border disabled:text-muted-foreground"
        >
          <ArrowUp className="h-4 w-4" />
        </Button>
      </div>
    </div>
  );
}
