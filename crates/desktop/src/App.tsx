import { useCallback, useEffect, useMemo, useState } from 'react';
import { Settings } from 'lucide-react';
import { Toaster } from 'sonner';
import { Sidebar } from './components/Sidebar';
import { MessageList } from './components/MessageList';
import { ChatInput } from './components/ChatInput';
import { ModelSelect } from './components/ModelSelect';
import { ThinkingSelect } from './components/ThinkingSelect';
import { SettingsModal } from './components/SettingsModal';
import { Button } from './components/ui/button';
import {
  useSessionStore,
  attachmentsFor,
  displayTitle,
  draftFor,
  isBusy,
  isReopening,
} from './lib/sessionStore';
import { useProviderStore, draftModelOptions, draftThinkingOptions } from './lib/providers';
import { useTheme, useThemeStore } from './lib/theme';
import { actionForEvent, useShortcutStore } from './lib/shortcuts';
import { suppressSelectAll, suppressWebviewContextMenu } from './lib/webview';
import { preserveMarkdownWhenCopying } from './lib/markdownSource';
import type { Provider } from './lib/acp';
import type { UsageState } from './lib/types';
import { t } from './lib/i18n';
import './styles.css';

function App() {
  const sessions = useSessionStore((s) => s.sessions);
  const activeId = useSessionStore((s) => s.activeId);
  const deletingIds = useSessionStore((s) => s.deletingIds);
  const unreadIds = useSessionStore((s) => s.unreadIds);
  const select = useSessionStore((s) => s.select);
  const addSession = useSessionStore((s) => s.addSession);
  const sendTurn = useSessionStore((s) => s.sendTurn);
  const queueFollowup = useSessionStore((s) => s.queueFollowup);
  const steerFollowup = useSessionStore((s) => s.steerFollowup);
  const removeFollowup = useSessionStore((s) => s.removeFollowup);
  const stopTurn = useSessionStore((s) => s.stopTurn);
  const regenerateTurn = useSessionStore((s) => s.regenerateTurn);
  const setModel = useSessionStore((s) => s.setModel);
  const setThinking = useSessionStore((s) => s.setThinking);
  const renameSession = useSessionStore((s) => s.renameSession);
  const deleteSession = useSessionStore((s) => s.deleteSession);
  const init = useSessionStore((s) => s.init);
  const initError = useSessionStore((s) => s.initError);
  const setDraft = useSessionStore((s) => s.setDraft);
  const setAttachments = useSessionStore((s) => s.setAttachments);
  const draft = useSessionStore((s) => draftFor(s, activeId));
  const attachments = useSessionStore((s) => attachmentsFor(s, activeId));
  const reopening = useSessionStore(isReopening);
  const providers = useProviderStore((s) => s.providers);
  const defaultModel = useProviderStore((s) => s.defaultModel);
  const bindings = useShortcutStore((s) => s.bindings);
  const [settingsOpen, setSettingsOpen] = useState(false);
  // The composer floats over the conversation, so the list keeps room under its
  // last message for it. Measured rather than fixed: queued messages make the
  // composer taller than any static padding accounts for. The inset adds 38 on
  // top of the measured height: the box's 18px offset from the bottom edge plus
  // 20px of clearance above it. 140 is the bare composer's own height (102)
  // plus that 38, so the layout holds steady until the observer reports a real
  // measurement.
  const [composerHeight, setComposerHeight] = useState(140);
  const onComposerHeight = useCallback((height: number) => setComposerHeight(height), []);

  useTheme();
  const theme = useThemeStore((s) => s.mode);

  useEffect(() => {
    void init();
  }, [init]);

  // The window's shortcuts, dispatched from one place: the settings panel only
  // writes bindings, so what it shows and what the window does cannot drift.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // A control that already claimed the key keeps it, a held key counts once,
      // and a settings row recording a binding must not fire the one it replaces.
      if (event.defaultPrevented || event.repeat || useShortcutStore.getState().recording) return;
      const id = actionForEvent(bindings, event);
      if (!id) return;
      event.preventDefault();
      if (id === 'session.new') addSession();
      else setSettingsOpen(true);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [bindings, addSession]);

  // The window is a webview, and its context menu and select-all are the
  // browser's rather than the app's. The menu is cancelled everywhere; text
  // fields keep select-all because the shortcut belongs to the draft. Copy is
  // left to the browser except inside a message, where the selection is replaced
  // with the markdown it came from.
  useEffect(() => {
    const restoreContextMenu = suppressWebviewContextMenu();
    const restoreSelectAll = suppressSelectAll();
    const restoreCopy = preserveMarkdownWhenCopying();
    return () => {
      restoreContextMenu();
      restoreSelectAll();
      restoreCopy();
    };
  }, []);

  // The catalog the ring needs is the provider store's, which the settings
  // panel only asks for when it opens; the main window wants it from the start.
  useEffect(() => {
    void useProviderStore.getState().load();
  }, []);

  const active = sessions.find((s) => s.id === activeId);

  // A draft has no agent session, so it has no selector of its own yet. The
  // provider catalog is what the agent would build one from, and showing it now
  // is how the user can see — and choose — the model the first message will run
  // on. Memoized because `ModelSelect` closes on a new options object, and a
  // draft re-renders for reasons that do not change its choice.
  const isDraft = active?.backendId === null;
  const draftChoice = isDraft ? (active?.model?.currentValue ?? defaultModel) : null;
  const draftOptions = useMemo(
    () => (isDraft ? draftModelOptions(providers, draftChoice) : null),
    [isDraft, draftChoice, providers],
  );
  const modelOptions = isDraft ? draftOptions : (active?.model ?? null);

  // A draft's thinking choices come from the catalog too, so a level the model
  // does not declare is never offered. A level already chosen is kept when the
  // model still declares it, and falls back to that model's default when not.
  const draftThinking = useMemo(
    () =>
      isDraft
        ? draftThinkingOptions(
            providers,
            draftChoice ?? '',
            active?.thinking?.currentValue ?? null,
          )
        : null,
    [isDraft, draftChoice, active?.thinking?.currentValue, providers],
  );
  const thinkingOptions = isDraft ? draftThinking : (active?.thinking ?? null);

  // The ring shows context pressure from the moment the session names a model,
  // not only after the first report: the window comes from the catalog, and
  // occupancy reads as zero until the agent says otherwise. Once reports
  // arrive they speak for themselves, window included.
  const modelRef = active?.model?.currentValue || defaultModel || '';
  const windowSize = contextWindowOf(providers, modelRef);
  const usage: UsageState | undefined =
    active?.usage ??
    (windowSize
      ? {
          used: 0,
          size: windowSize,
          inputTokens: 0,
          outputTokens: 0,
          cachedInputTokens: 0,
          cacheWriteInputTokens: 0,
          reasoningOutputTokens: 0,
        }
      : undefined);

  return (
    <div className="flex h-full">
      <Sidebar
        sessions={sessions}
        activeId={activeId}
        deletingIds={deletingIds}
        unreadIds={unreadIds}
        onSelect={select}
        onNew={addSession}
        onRename={renameSession}
        onDelete={deleteSession}
      />
      <main className="relative flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-border px-5 text-sm font-medium">
          <span className="truncate">{active ? displayTitle(active) : 'SrudAgent'}</span>
          <div className="flex shrink-0 items-center gap-1">
            {/* The session's model, next to the session it changes. A draft has
                no agent selector yet and is shown the catalog's; a session the
                agent offers no choice for has none at all. */}
            <ModelSelect
              options={modelOptions}
              disabled={reopening}
              onSelect={setModel}
            />
            {/* The level the selected model runs at. A model that declares no
                thinking control has nothing to pick, and the selector hides. */}
            <ThinkingSelect
              options={thinkingOptions}
              disabled={reopening}
              onSelect={setThinking}
            />
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setSettingsOpen(true)}
              title={t('settings.open')}
              aria-label={t('settings.open')}
            >
              <Settings className="h-4 w-4" />
            </Button>
          </div>
        </header>
        {initError && (
          <div className="border-b border-border bg-destructive/10 px-5 py-2 text-xs text-destructive">
            {initError}
          </div>
        )}
        <MessageList
          turns={active?.turns ?? []}
          reopening={reopening}
          bottomInset={composerHeight + 38}
          onRegenerate={regenerateTurn}
        />
        {/* Busy while a turn runs, and while a session is still being reopened:
            the conversation is arriving then, so a prompt would interleave with
            it. There is no turn to stop in that window, so the stop control does
            nothing until one is open. */}
        <ChatInput
          value={draft}
          onValueChange={(text) => setDraft(activeId, text)}
          attachments={attachments}
          onAttachmentsChange={(images) => setAttachments(activeId, images)}
          onSend={(text, images) => sendTurn(text, undefined, images)}
          onQueue={queueFollowup}
          followups={active?.followups}
          onSteerFollowup={steerFollowup}
          onRemoveFollowup={removeFollowup}
          busy={isBusy(active) || reopening}
          onStop={stopTurn}
          usage={usage}
          onHeightChange={onComposerHeight}
        />
      </main>
      <SettingsModal open={settingsOpen} onClose={() => setSettingsOpen(false)} />
      <Toaster theme={theme} position="top-center" />
    </div>
  );
}

// The configured window of the model a reference names, or undefined when the
// reference is empty, malformed, or names a model that is gone. A reference is
// spelled `providerId/modelId`, and model ids never carry a slash.
function contextWindowOf(providers: Provider[], reference: string): number | undefined {
  const slash = reference.lastIndexOf('/');
  if (slash <= 0) return undefined;
  const provider = providers.find((p) => p.id === reference.slice(0, slash));
  return provider?.models.find((m) => m.id === reference.slice(slash + 1))?.contextWindow;
}

export default App;
