import { Sidebar } from './components/Sidebar';
import { MessageList } from './components/MessageList';
import { ChatInput } from './components/ChatInput';
import { useSessionStore } from './lib/sessionStore';
import './styles.css';

function App() {
  const sessions = useSessionStore((s) => s.sessions);
  const activeId = useSessionStore((s) => s.activeId);
  const select = useSessionStore((s) => s.select);
  const addSession = useSessionStore((s) => s.addSession);
  const sendTurn = useSessionStore((s) => s.sendTurn);

  const active = sessions.find((s) => s.id === activeId);

  return (
    <div className="flex h-full">
      <Sidebar sessions={sessions} activeId={activeId} onSelect={select} onNew={addSession} />
      <main className="relative flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center border-b border-border px-5 text-sm font-medium">
          {active?.title ?? 'SrudAgent'}
        </header>
        <MessageList turns={active?.turns ?? []} />
        <ChatInput onSend={sendTurn} onNew={addSession} />
      </main>
    </div>
  );
}

export default App;
