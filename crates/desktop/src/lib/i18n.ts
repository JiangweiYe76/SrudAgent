export type Locale = 'en';

const en = {
  'sidebar.newChat': 'New chat',
  'sidebar.backendLive': 'backend: live',
  'sidebar.backendMock': 'backend: mock',
  'sidebar.renameTitle': 'Rename session',
  'sidebar.renamePlaceholder': 'Session name',
  'sidebar.renameSave': 'Save',
  'sidebar.deleteTitle': 'Delete session',
  'sidebar.deleteWarning': 'Delete "{title}"? This cannot be undone.',
  'sidebar.deleteWarningWithTurns':
    'Delete "{title}" and its {n} message(s)? This cannot be undone.',
  'sidebar.deleteConfirm': 'Delete',
  'sidebar.actions': 'Session actions',

  'time.justNow': 'just now',
  'time.minutesAgo': '{n} min ago',
  'time.hoursAgo': '{n} h ago',
  'time.daysAgo': '{n} d ago',

  'app.newSession': 'New session',

  'empty.startChat': 'Start a new chat',

  'toolCall.noResult': '(no result)',

  'thought.title': 'Thinking',

  'turn.running': 'Working…',
  'turn.ended': 'turn {reason}',

  'msg.copy': 'Copy',

  'common.close': 'Close',
  'common.cancel': 'Cancel',

  'settings.open': 'Settings',
  'settings.title': 'Settings',
  'settings.appearance': 'Appearance',
  'settings.about': 'About',
  'settings.theme': 'Theme',
  'settings.theme.hint': 'Follows your system preference unless you pick a side.',
  'settings.theme.system': 'System',
  'settings.theme.light': 'Light',
  'settings.theme.dark': 'Dark',
  'settings.about.line': 'SrudAgent 0.1.0 — an AI agent runtime in Rust.',

  'chat.placeholder': 'Type a message… (Enter to send, Shift+Enter for newline)',
  'chat.send': 'Send',
  'chat.stop': 'Stop',

  'mock.reply': '(mock) Got it. Real replies will appear here once the backend is wired up.',
} as const;

type Key = keyof typeof en;

const dictionaries: Record<Locale, Record<Key, string>> = { en };

const current: Locale = 'en';

type Params = Record<string, string | number>;

export function t(key: Key, params?: Params): string {
  let s = dictionaries[current][key];
  for (const [k, v] of Object.entries(params ?? {})) {
    s = s.split(`{${k}}`).join(String(v));
  }
  return s;
}
