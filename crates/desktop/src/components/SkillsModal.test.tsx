// @vitest-environment jsdom
//
// The modal only reads: it asks the agent for the skills visible from the
// active workspace, then reads the selected one. The tests cover a populated
// list with its detail pane and conflict banner, and an empty workspace.
import {
  describe,
  it,
  expect,
  beforeAll,
  afterEach,
  beforeEach,
  vi,
} from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { SkillsModal } from './SkillsModal';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));

let container: HTMLDivElement;
let root: Root;

function mount(): void {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
}

async function render(open: boolean): Promise<void> {
  act(() => {
    root.render(<SkillsModal open={open} cwd="" onClose={() => {}} />);
  });
  // The list is one request and the selected skill is a second; two macrotasks
  // let both mocked replies settle.
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

beforeAll(() => {
  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe('SkillsModal', () => {
  it('lists skills and shows the selected skill in detail', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({
        jsonrpc: '2.0',
        id: 1,
        result: {
          skills: [
            {
              name: 'pdf-tools',
              description: 'Read and convert PDFs.',
              path: '/workspace/.srud/skills/pdf-tools/SKILL.md',
              modelInvocable: true,
              userInvocable: true,
            },
            {
              name: 'private-notes',
              description: 'Notes the model must not see.',
              path: '/workspace/.srud/skills/private-notes/SKILL.md',
              modelInvocable: false,
              userInvocable: true,
            },
          ],
          conflicts: [
            {
              name: 'pdf-tools',
              winnerPath: '/workspace/.srud/skills/pdf-tools/SKILL.md',
              loserPath: '/user/.agents/skills/pdf-tools/SKILL.md',
            },
          ],
        },
      })
      .mockResolvedValueOnce({
        jsonrpc: '2.0',
        id: 2,
        result: {
          skill: {
            name: 'pdf-tools',
            description: 'Read and convert PDFs.',
            path: '/workspace/.srud/skills/pdf-tools/SKILL.md',
            modelInvocable: true,
            userInvocable: true,
            license: 'MIT',
            compatibility: 'Linux',
            allowedTools: ['Bash', 'Read'],
            metadata: { author: 'example' },
            body: '# PDF tools\n\nRead `references/formats.md`.',
            supportingFiles: ['references/formats.md', 'scripts/extract.sh'],
          },
        },
      });

    mount();
    await render(true);

    expect(document.body.textContent).toContain('pdf-tools');
    expect(document.body.textContent).toContain('private-notes');
    expect(document.body.textContent).toContain('MIT');
    expect(document.body.textContent).toContain('Bash · Read');
    expect(document.body.textContent).toContain('scripts/extract.sh');
    expect(document.body.textContent).toContain('references/formats.md');
    expect(document.body.textContent).toContain('1 name conflict(s)');
    expect(document.body.textContent).toContain(
      '/user/.agents/skills/pdf-tools/SKILL.md',
    );
  });

  it('says so when the workspace has no skills', async () => {
    vi.mocked(invoke).mockResolvedValue({
      jsonrpc: '2.0',
      id: 1,
      result: { skills: [] },
    });

    mount();
    await render(true);

    expect(document.body.textContent).toContain(
      'No skills found for this workspace.',
    );
  });
});
