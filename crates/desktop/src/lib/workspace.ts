// The directory a session works in, as the user chooses it.
//
// A session's `cwd` is fixed once the agent creates it, so the choice is made
// while the session is still a draft. The empty string means "not chosen": the
// agent gives that session a workspace of its own under its configuration
// directory.

import { open } from '@tauri-apps/plugin-dialog';
import { isTauri } from './acp';
import { t } from './i18n';

/**
 * Opens the platform's directory picker.
 *
 * Answers the chosen absolute path, or null when the user closed the picker
 * without choosing. Outside the desktop shell there is no picker to open, and
 * the app treats that the same way as a dismissed one.
 */
export async function pickWorkspace(): Promise<string | null> {
  if (!isTauri()) return null;
  const picked = await open({
    directory: true,
    multiple: false,
    title: t('workspace.choose'),
  });
  return typeof picked === 'string' ? picked : null;
}

/** The name a workspace goes by: its last path segment, or the default label. */
export function workspaceName(path: string): string {
  if (!path) return t('workspace.default');
  const trimmed = path.replace(/[\\/]+$/, '');
  const cut = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'));
  return trimmed.slice(cut + 1) || trimmed;
}

/**
 * Whether the path is a workspace the agent made, rather than one the user
 * chose.
 *
 * The agent creates those directly under its own `workspaces` directory, one
 * per session, each named after the session it belongs to.
 */
export function isAgentWorkspace(path: string): boolean {
  return /(^|[\\/])workspaces[\\/][^\\/]+$/.test(path.replace(/[\\/]+$/, ''));
}
