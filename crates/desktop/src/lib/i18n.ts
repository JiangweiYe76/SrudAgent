// Minimal i18n layer. To add a locale, extend `dictionaries` and switch `current`.

export type Locale = 'en';

const en = {
  'sidebar.newChat': 'New chat',
  'sidebar.backendMock': 'backend: mock',

  'time.justNow': 'just now',
  'time.minutesAgo': '{n} min ago',
  'time.hoursAgo': '{n} h ago',
  'time.daysAgo': '{n} d ago',

  'app.newSession': 'New session',

  'empty.startChat': 'Start a new chat',

  'toolCall.noResult': '(no result)',

  'turn.running': 'Working…',
  'turn.ended': 'turn {reason}',

  'msg.copy': 'Copy',

  'chat.placeholder': 'Type a message… (Enter to send, Shift+Enter for newline)',
  'chat.send': 'Send',

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
