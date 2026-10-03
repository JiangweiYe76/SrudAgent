import { useEffect, useState } from 'react';
import { Settings } from 'lucide-react';
import { Sidebar } from './components/Sidebar';
import { MessageList } from './components/MessageList';
import { ChatInput } from './components/ChatInput';
import { SettingsModal } from './components/SettingsModal';
import { Button } from './components/ui/button';
import { useSessionStore, displayTitle, isBusy } from './lib/sessionStore';
import { useTheme } from './lib/theme';
import { t } from './lib/i18n';
import './styles.css';

function App() {
  const sessions = useSessionStore((s) => s.sessions);
  const activeId = useSessionStore((s) => s.activeId);
  const select = useSessionStore((s) => s.select);
  const addSession = useSessionStore((s) => s.addSession);
  const sendTurn = useSessionStore((s) => s.sendTurn);
  const stopTurn = useSessionStore((s) => s.stopTurn);
  const renameSession = useSessionStore((s) => s.renameSession);
  const deleteSession = useSessionStore((s) => s.deleteSession);
  const init = useSessionStore((s) => s.init);
  const initError = useSessionStore((s) => s.initError);
  const reopening = useSessionStore((s) => s.reopening);
  const [settingsOpen, setSettingsOpen] = useState(false);

  useTheme();

  useEffect(() => {
    void init();
  }, [init]);

  const active = sessions.find((s) => s.id === activeId);

  return (
    <div className="flex h-full">
      <Sidebar
        sessions={sessions}
        activeId={activeId}
        connected={sessions.length > 0 && !initError}
        onSelect={select}
        onNew={addSession}
        onRename={renameSession}
        onDelete={deleteSession}
      />
      <main className="relative flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-border px-5 text-sm font-medium">
          <span className="truncate">{active ? displayTitle(active) : 'SrudAgent'}</span>
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
        {initError && (
          <div className="border-b border-border bg-destructive/10 px-5 py-2 text-xs text-destructive">
            {initError}
          </div>
        )}
        <MessageList turns={active?.turns ?? []} />
        {/* Busy while a turn runs, and while a session is still being reopened:
            the conversation is arriving then, so a prompt would interleave with
            it. There is no turn to stop in that window, so the stop control does
            nothing until one is open. */}
        <ChatInput
          onSend={sendTurn}
          onNew={addSession}
          busy={isBusy(active) || reopening}
          onStop={stopTurn}
        />
      </main>
      <SettingsModal open={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </div>
  );
}

export default App;
