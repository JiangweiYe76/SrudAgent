export type Locale = 'en';

const en = {
  'sidebar.newChat': 'New chat',
  'sidebar.renameTitle': 'Rename session',
  'sidebar.renamePlaceholder': 'Session name',
  'sidebar.renameSave': 'Save',
  'sidebar.forkTitle': 'Fork session',
  'sidebar.forkFailed': 'Could not fork the session.',
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
  'sidebar.streaming': 'Still replying…',
  'sidebar.unread': 'Unread reply',

  'skills.open': 'Skills',
  'skills.title': 'Skills',
  'skills.loading': 'Loading skills…',
  'skills.empty': 'No skills found for this workspace.',
  'skills.emptyHint': 'Add a SKILL.md under .srud/skills in this workspace.',
  'skills.loadFailed': 'Could not load skills.',
  'skills.select': 'Select a skill to see its details.',
  'skills.detailLoading': 'Loading skill…',
  'skills.detailFailed': 'Could not load this skill.',
  'skills.conflicts': '{n} name conflict(s)',
  'skills.conflictsHint': 'The higher-priority skill wins.',
  'skills.hidden': 'Hidden from model',
  'skills.modelOnly': 'Model only',
  'skills.fields': 'Details',
  'skills.license': 'License',
  'skills.compatibility': 'Compatibility',
  'skills.allowedTools': 'Allowed tools',
  'skills.visibility': 'Visibility',
  'skills.modelVisible': 'Model visible',
  'skills.userVisible': 'User invocable',
  'skills.metadata': 'Metadata',
  'skills.files': 'Bundled files',
  'skills.noFiles': 'No bundled files.',
  'skills.instructions': 'Instructions',
  'skills.noInstructions': 'No instructions.',

  'session.repairFailed':
    'Part of this conversation was lost while loading and could not be restored.',
  'session.settingFailed': 'Could not change this setting.',
  'session.renameFailed': 'Could not rename the session.',
  'session.steerFailed': 'Could not send that message.',

  'time.justNow': 'just now',
  'time.minutesAgo': '{n} min ago',
  'time.hoursAgo': '{n} h ago',
  'time.daysAgo': '{n} d ago',

  'app.newSession': 'New session',
  'app.retry': 'Retry',

  'sandbox.fullAccess': 'Full access',
  'sandbox.workspaceWrite': 'Workspace write',
  'sandbox.fullAccessHint':
    'The agent can read and write any file this account can. Commands run without an OS sandbox.',
  'sandbox.fullAccessTitle': 'Enable full access?',
  'sandbox.fullAccessScope': 'Applies to this session only.',
  'sandbox.fullAccessConsequence':
    'The agent can read and write any file this account can.',
  'sandbox.fullAccessExit':
    'Switch back to Workspace write, or end the session, to leave this mode.',
  'sandbox.enableFullAccess': 'Enable full access',

  'empty.startChat': 'Start a new chat',
  'empty.loadingSession': 'Loading conversation…',

  'toolCall.noResult': '(no result)',
  'toolCall.input': 'Input',
  'toolCall.output': 'Output',
  'toolCall.unexpectedText': 'not JSON',
  'toolCall.viewFull': 'View full content',

  'thought.title': 'Thinking',

  'turn.awaitingReply': 'Waiting for a reply',
  'turn.ended': 'turn {reason}',
  'turn.regenerate': 'Regenerate',
  'turn.retry': 'Retry',
  'turn.retryFailed': 'Could not re-answer this turn.',
  'turn.stopFailed': 'Could not stop the turn.',
  'turn.error.title': 'The model request failed',
  'turn.error.authTitle': 'Provider sign-in required',
  'turn.error.rateTitle': 'Too many requests',
  'turn.error.serverTitle': 'The provider returned an error',
  'turn.error.contextTitle': 'The conversation is too long',
  'turn.error.networkTitle': 'Connection failed',
  'turn.error.unknownTitle': 'This turn could not finish',
  'turn.error.details': 'Details',
  'turn.error.detailsTitle': 'Error details',
  'turn.error.code': 'Code',
  'turn.error.message': 'Message',

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
  'common.back': 'Back',
  'common.delete': 'Delete',
  'common.reveal': 'Show value',
  'common.conceal': 'Hide value',

  'settings.open': 'Settings',
  'settings.title': 'Settings',
  'settings.appearance': 'Appearance',
  'settings.models': 'Models',
  'settings.about': 'About',
  'settings.theme': 'Theme',
  'settings.theme.hint':
    'Follows your system preference unless you pick a side.',
  'settings.theme.system': 'System',
  'settings.theme.light': 'Light',
  'settings.theme.dark': 'Dark',
  'settings.zoom': 'Zoom',
  'settings.zoom.hint':
    'Scales the whole window — text, icons and spacing together.',
  'settings.shortcuts': 'Shortcuts',

  'shortcuts.hint':
    'Click a binding and press the keys to replace it. Escape cancels.',
  'shortcuts.session.new': 'New session',
  'shortcuts.settings.open': 'Open settings',
  'shortcuts.recording': 'Press keys…',
  'shortcuts.needModifier':
    'Hold Ctrl, Alt or Shift as well — a key on its own would fire while typing.',
  'shortcuts.taken': 'Already bound to “{name}”.',
  'shortcuts.reset': 'Reset',
  'settings.about.line': 'SrudAgent 0.1.0 — an AI agent runtime in Rust.',

  'models.unavailable': '{model} (unavailable)',
  'models.choose': 'Choose a model',

  'thinking.title': 'Thinking level',
  'thinking.choose': 'Thinking',

  'providers.title': 'Providers',
  'providers.hint': 'Endpoints your sessions can draw models from.',
  'providers.loading': 'Loading…',
  'providers.empty':
    'No provider is configured. Add one to give sessions a model.',
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
  'providers.field.id': 'ID',
  'providers.field.name': 'Name',
  'providers.field.baseUrl': 'Base URL',
  'providers.field.apiKey': 'API key',
  'providers.field.apiKey.keep':
    'Leave it as it is to keep the stored key; clear it to remove the key.',
  'providers.field.apiKey.new': 'Stored on this machine, readable only by you.',
  'providers.models.manage': 'Manage models',
  'providers.models.title': 'Models of {name}',
  'providers.models.of': '{id}',
  'providers.model.add': 'Add model',
  'providers.model.empty':
    'Nothing yet. Without a model this provider cannot be used.',
  'providers.model.edit': 'Edit model',
  'providers.model.remove': 'Remove model',
  'providers.model.form.add': 'Add a model',
  'providers.model.form.edit': 'Editing {id}',
  'providers.model.field.id': 'Model ID',
  'providers.model.field.name': 'Label',
  'providers.model.field.name.hint':
    'What the picker shows. The ID stands in when there is none.',
  'providers.model.field.accepts': 'Accepts',
  'providers.model.field.accepts.hint':
    'Besides text. Nothing selected is a text-only model.',
  'providers.model.field.context': 'Context window',
  'providers.model.field.context.hint':
    'Maximum tokens in context, as tokens. Blank hides the usage ring.',
  'providers.model.badContext':
    'The context window must be a whole number of tokens above zero.',
  'providers.model.field.thinking': 'Thinking',
  'providers.model.thinking.default': 'Default level',
  'providers.model.thinking.levels': 'Levels',
  'providers.model.thinking.level': 'Level',
  'providers.model.thinking.add': 'Add level',
  'providers.model.thinking.remove': 'Remove level',
  'providers.model.thinking.choose': 'Choose a level',
  'providers.model.thinking.badValue':
    'Every level needs the value the endpoint receives.',
  'providers.model.thinking.badDuplicate':
    'Each level may only be listed once.',
  'providers.model.thinking.badDefault':
    'The default must be one of the listed levels.',
  'providers.modality.image': 'Image',
  'providers.modality.audio': 'Audio',
  'providers.modality.video': 'Video',
  'providers.model.duplicate': 'This provider already offers `{id}`.',
  'providers.key.set': 'key {key}',
  'providers.key.none': 'no key',

  // What a refused change says. The agent names the rule and the values; the
  // wording is here so it can be written in the reader's language.
  'providers.error.invalidId':
    'The ID must be lowercase letters, digits, `-` or `_`, and start with a letter or digit.',
  'providers.error.blankName': 'A provider needs a name.',
  'providers.error.badBaseUrl':
    'The base URL must be an absolute HTTP or HTTPS URL with no trailing slash.',
  'providers.error.blankModelId': 'A model needs an ID.',
  'providers.error.duplicateProvider': 'The provider ID `{id}` is used twice.',
  'providers.error.thinkingLevelsMissing':
    'The model `{model}` declares thinking but lists no levels.',
  'providers.error.thinkingDefaultMissing':
    'The model `{model}` defaults to thinking level `{level}`, which it does not list.',
  'providers.error.thinkingBlankValue':
    'The model `{model}` maps thinking level `{level}` to a blank value.',
  'providers.error.unknownProvider':
    'No provider with the ID `{id}` exists any more.',
  'providers.error.unknownModel': 'No model `{model}` exists any more.',
  'providers.error.badReference':
    '`{reference}` is not a `provider/model` reference.',

  'chat.placeholder':
    'Type a message… (Enter to send, Shift+Enter for newline)',
  'chat.send': 'Send',
  'chat.scrollToBottom': 'Scroll to bottom',
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

  'mock.reply':
    '(mock) Got it. Real replies will appear here once the backend is wired up.',
} as const;

export type Key = keyof typeof en;

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
