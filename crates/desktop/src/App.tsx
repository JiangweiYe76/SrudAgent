import { useState } from 'react';
import { Settings } from 'lucide-react';
import { Sidebar } from './components/Sidebar';
import { MessageList } from './components/MessageList';
import { ChatInput } from './components/ChatInput';
import { SettingsModal } from './components/SettingsModal';
import { Button } from './components/ui/button';
import { useSessionStore } from './lib/sessionStore';
import { useTheme } from './lib/theme';
import { t } from './lib/i18n';
import './styles.css';

function App() {
  const sessions = useSessionStore((s) => s.sessions);
  const activeId = useSessionStore((s) => s.activeId);
  const select = useSessionStore((s) => s.select);
  const addSession = useSessionStore((s) => s.addSession);
  const sendTurn = useSessionStore((s) => s.sendTurn);
  const [settingsOpen, setSettingsOpen] = useState(false);

  useTheme();

  const active = sessions.find((s) => s.id === activeId);

  return (
    <div className="flex h-full">
      <Sidebar sessions={sessions} activeId={activeId} onSelect={select} onNew={addSession} />
      <main className="relative flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-border px-5 text-sm font-medium">
          <span className="truncate">{active?.title ?? 'SrudAgent'}</span>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => setSettingsOpen(true)}
            title={t('settings.open')}
            aria-label={t('settings.open')}
          >
            <Settings className="h-4 w-4" />
          </Button>
        </header>
        <MessageList turns={active?.turns ?? []} />
        <ChatInput onSend={sendTurn} onNew={addSession} />
      </main>
      <SettingsModal open={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </div>
  );
}

export default App;
