export type Locale = 'en';

const en = {
  'sidebar.newChat': 'New chat',
  'sidebar.renameTitle': 'Rename session',
  'sidebar.renamePlaceholder': 'Session name',
  'sidebar.renameSave': 'Save',
  'sidebar.deleteTitle': 'Delete session',
  'sidebar.deleteWarning': 'Delete "{title}"? This cannot be undone.',
  'sidebar.deleteWarningWithTurns':
    'Delete "{title}" and its {n} message(s)? This cannot be undone.',
  'sidebar.deleteConfirm': 'Delete',
  'sidebar.actions': 'Session actions',
  'sidebar.deleting': 'Deleting…',
  'sidebar.deleted': 'Session deleted',
  'sidebar.deleteFailed': 'Could not delete the session.',
  'sidebar.deleteBusy': 'Stop the running turn before deleting this session.',

  'time.justNow': 'just now',
  'time.minutesAgo': '{n} min ago',
  'time.hoursAgo': '{n} h ago',
  'time.daysAgo': '{n} d ago',

  'app.newSession': 'New session',

  'empty.startChat': 'Start a new chat',
  'empty.loadingSession': 'Loading conversation…',

  'toolCall.noResult': '(no result)',
  'toolCall.input': 'Input',
  'toolCall.output': 'Output',
  'toolCall.unexpectedText': 'not JSON',

  'thought.title': 'Thinking',

  'turn.awaitingReply': 'Waiting for a reply',
  'turn.ended': 'turn {reason}',

  'msg.copy': 'Copy',
  'msg.copied': 'Copied',
  'msg.actions': 'Message actions',

  'edit.actions': 'Editing actions',
  'edit.cut': 'Cut',
  'edit.copy': 'Copy',
  'edit.paste': 'Paste',
  'edit.selectAll': 'Select all',

  'code.copy': 'Copy code',
  'code.copied': 'Copied',
  'code.plainText': 'text',

  'mermaid.rendering': 'Rendering diagram…',
  'mermaid.generating': 'Generating diagram…',
  'mermaid.failed': 'Could not render this diagram.',
  'mermaid.showCode': 'Show code',
  'mermaid.showDiagram': 'Show diagram',

  'common.close': 'Close',
  'common.cancel': 'Cancel',
  'common.delete': 'Delete',
  'common.reveal': 'Show value',
  'common.conceal': 'Hide value',

  'settings.open': 'Settings',
  'settings.title': 'Settings',
  'settings.appearance': 'Appearance',
  'settings.models': 'Models',
  'settings.about': 'About',
  'settings.theme': 'Theme',
  'settings.theme.hint': 'Follows your system preference unless you pick a side.',
  'settings.theme.system': 'System',
  'settings.theme.light': 'Light',
  'settings.theme.dark': 'Dark',
  'settings.zoom': 'Zoom',
  'settings.zoom.hint': 'Scales the whole window — text, icons and spacing together.',
  'settings.about.line': 'SrudAgent 0.1.0 — an AI agent runtime in Rust.',

  'models.unavailable': '{model} (unavailable)',
  'models.choose': 'Choose a model',

  'providers.title': 'Providers',
  'providers.hint': 'Endpoints your sessions can draw models from.',
  'providers.loading': 'Loading…',
  'providers.empty': 'No provider is configured. Add one to give sessions a model.',
  'providers.modelCount': '{n} model(s)',
  'providers.add': 'Add provider',
  'providers.edit': 'Edit provider',
  'providers.delete': 'Delete provider',
  'providers.default': 'Default model',
  'providers.default.hint':
    'What new sessions start from. Sessions already open keep their own.',
  'providers.default.unset': 'Not chosen',
  'providers.form.add': 'Add a provider',
  'providers.form.edit': 'Editing {id}',
  'providers.form.save': 'Save',
  'providers.form.models.hint':
    'This provider offers {n} model(s). They are managed from its row in the list.',
  'providers.field.id': 'Id',
  'providers.field.name': 'Name',
  'providers.field.baseUrl': 'Base URL',
  'providers.field.apiKey': 'API key',
  'providers.field.apiKey.keep': 'Leave it as it is to keep the stored key; clear it to remove the key.',
  'providers.field.apiKey.new': 'Stored on this machine, readable only by you.',
  'providers.models.manage': 'Manage models',
  'providers.models.title': 'Models of {name}',
  'providers.models.hint': 'What this endpoint can be asked for.',
  'providers.models.of': '{id}',
  'providers.model.add': 'Add model',
  'providers.model.empty': 'Nothing yet. Without a model this provider cannot be used.',
  'providers.model.edit': 'Edit model',
  'providers.model.remove': 'Remove model',
  'providers.model.form.add': 'Add a model',
  'providers.model.form.edit': 'Editing {id}',
  'providers.model.field.id': 'Model id',
  'providers.model.field.name': 'Label',
  'providers.model.field.name.hint': 'What the picker shows. The id stands in when there is none.',
  'providers.model.field.accepts': 'Accepts',
  'providers.model.field.accepts.hint': 'Besides text. Nothing selected is a text-only model.',
  'providers.model.field.context': 'Context window',
  'providers.model.field.context.hint':
    'Maximum tokens in context, as tokens. Blank hides the usage ring.',
  'providers.model.badContext': 'The context window must be a whole number of tokens above zero.',
  'providers.modality.image': 'Image',
  'providers.modality.audio': 'Audio',
  'providers.modality.video': 'Video',
  'providers.model.badId': "A model id must not contain `/` — it is what separates a model from its provider.",
  'providers.model.duplicate': 'This provider already offers `{id}`.',
  'providers.key.set': 'key {key}',
  'providers.key.none': 'no key',

  'chat.placeholder': 'Type a message… (Enter to send, Shift+Enter for newline)',
  'chat.send': 'Send',
  'chat.steer': 'Steer',
  'chat.stop': 'Stop',
  'chat.context': 'Context: {pct}% ({used} / {size} tokens)',
  'chat.image.remove': 'Remove image',
  'chat.image.preview': 'Image preview',
  'chat.image.tooLarge': 'Images over 10 MB cannot be attached.',
  'chat.image.unreadable': 'Could not read that image.',
  'usage.title': 'Token usage',
  'usage.context': 'Context',
  'usage.cacheHit': 'Cache hit',
  'usage.input': 'Input tokens',
  'usage.output': 'Output tokens',
  'usage.cached': 'Cached input',
  'usage.cacheWrite': 'Cache write',
  'usage.reasoning': 'Reasoning output',
  'usage.note': 'Totals accumulate over every step this session reported.',

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
