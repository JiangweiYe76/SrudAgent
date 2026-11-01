// @vitest-environment jsdom
//
// Only the deleting row is at stake here: while a delete is in flight the row
// must visibly be on its way out — dimmed, with the actions it could otherwise
// re-fire swapped for a spinner — and the other rows must render exactly as
// they do without one. The activity dots ride along, since they share the row's
// status slot.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { Sidebar } from './Sidebar';
import type { Session } from '@/lib/types';

let container: HTMLDivElement;
let root: Root;

function mount(): void {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
}

function session(id: string): Session {
  const now = Date.now();
  return {
    id,
    backendId: null,
    cwd: '',
    loaded: true,
    title: null,
    model: null,
    turns: [],
    createdAt: now,
    updatedAt: now,
  };
}

function render(
  sessions: Session[],
  deletingIds: string[] = [],
  unreadIds: string[] = [],
  onFork: (id: string) => void = () => {},
): void {
  act(() => {
    root.render(
      <Sidebar
        sessions={sessions}
        activeId={sessions[0]?.id ?? ''}
        deletingIds={deletingIds}
        unreadIds={unreadIds}
        onSelect={() => {}}
        onNew={() => {}}
        onRename={() => {}}
        onFork={onFork}
        onDelete={() => {}}
      />,
    );
  });
}

/** The session rows, in order. */
function rows(): HTMLButtonElement[] {
  return Array.from(
    container.querySelectorAll<HTMLButtonElement>('nav > button'),
  );
}

beforeAll(() => {
  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe('Sidebar deleting rows', () => {
  it('marks the row a delete is in flight for', () => {
    mount();
    render([session('s1'), session('s2')], ['s1']);
    const [deleting, untouched] = rows();

    expect(deleting.getAttribute('aria-disabled')).toBe('true');
    expect(deleting.className).toContain('opacity-60');
    expect(deleting.querySelector('[role="status"]')).not.toBeNull();
    expect(deleting.querySelector('[title="Deleting…"]')).not.toBeNull();
    expect(deleting.querySelector('[aria-label="Session actions"]')).toBeNull();

    // The neighbour keeps its actions and stays selectable.
    expect(untouched.getAttribute('aria-disabled')).toBeNull();
    expect(untouched.className).not.toContain('opacity-60');
    expect(untouched.querySelector('[role="status"]')).toBeNull();
    expect(
      untouched.querySelector('[aria-label="Session actions"]'),
    ).not.toBeNull();
  });

  it('renders every row with its actions when no delete is in flight', () => {
    mount();
    render([session('s1'), session('s2')]);
    for (const row of rows()) {
      expect(row.getAttribute('aria-disabled')).toBeNull();
      expect(row.querySelector('[role="status"]')).toBeNull();
      expect(
        row.querySelector('[aria-label="Session actions"]'),
      ).not.toBeNull();
    }
  });

  it('marks a streaming row with a pulsing dot and an unread row with a plain one', () => {
    // An open last turn is all "streaming" means to the sidebar.
    const busy = session('s1');
    busy.turns = [
      { id: 't1', userInput: 'hi', steps: [], createdAt: Date.now() },
    ];
    mount();
    render([busy, session('s2')], [], ['s2']);
    const [streaming, unread] = rows();

    expect(streaming.querySelector('[title="Still replying…"]')).not.toBeNull();
    expect(unread.querySelector('[title="Unread reply"]')).not.toBeNull();
    // Streaming outranks unread, so one row never carries both.
    expect(streaming.querySelector('[title="Unread reply"]')).toBeNull();
    expect(unread.querySelector('[title="Still replying…"]')).toBeNull();
  });
});

describe('Sidebar fork action', () => {
  // Opens the row's actions menu the way a keyboard does it: the trigger is a
  // menu button, and jsdom has no pointer pipeline for the mouse path.
  function openActions(row: Element): void {
    const trigger = row.querySelector('[aria-label="Session actions"]');
    act(() => {
      (trigger as HTMLElement).focus();
      trigger!.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }),
      );
    });
  }

  function menuItem(text: string): Element | null {
    return (
      Array.from(document.body.querySelectorAll('[role="menuitem"]')).find(
        (item) => item.textContent?.includes(text),
      ) ?? null
    );
  }

  it('offers fork for a session the agent holds and reports its id', () => {
    mount();
    const source = { ...session('s1'), backendId: 'S1' };
    let forked: string | null = null;
    render([source], [], [], (id) => {
      forked = id;
    });

    openActions(rows()[0]);
    const item = menuItem('Fork session');
    expect(item).not.toBeNull();
    act(() => (item as HTMLElement).click());
    expect(forked).toBe('s1');
  });

  it('offers no fork for a draft, which holds nothing to copy', () => {
    mount();
    render([session('s1')]);
    openActions(rows()[0]);
    expect(menuItem('Fork session')).toBeNull();
    // Rename and delete are still there; only fork is held back.
    expect(menuItem('Rename session')).not.toBeNull();
    expect(menuItem('Delete session')).not.toBeNull();
  });
});
