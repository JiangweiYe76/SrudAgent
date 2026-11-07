// Conversations a mock preview shows in place of a real agent's. They exist so
// the error notice and the transcript limits can be seen without a backend;
// only the mock dev commands load them.
import type {
  ModelOptions,
  Session,
  Step,
  ToolCall,
  Turn,
  TurnError,
} from './types';
import { DEFAULT_SANDBOX_MODE } from './types';

const NOW = Date.now();
const MINUTE = 60_000;

const MODEL: ModelOptions = {
  configId: 'model',
  currentValue: 'kimi-k3',
  groups: [
    {
      id: 'bailian',
      name: 'bailian',
      choices: [{ value: 'kimi-k3', name: 'kimi-k3' }],
    },
  ],
};

const THINKING = {
  configId: 'thinking',
  currentValue: 'medium',
  choices: [
    { value: 'off', name: 'Off' },
    { value: 'medium', name: 'Medium' },
    { value: 'high', name: 'High' },
  ],
};

let sequence = 0;
function nextId(prefix: string): string {
  sequence += 1;
  return `${prefix}-mock-${sequence}`;
}

function failedTurn(
  userInput: string,
  error: TurnError | undefined,
  at: number,
  partialText = '',
): Turn {
  const steps: Step[] = partialText
    ? [{ id: nextId('step'), assistantText: partialText, toolCalls: [] }]
    : [];
  return {
    id: nextId('turn'),
    userInput,
    steps,
    endReason: 'error',
    ...(error ? { error } : {}),
    createdAt: at,
    endedAt: at + 1_000,
  };
}

function endedTurn(
  userInput: string,
  reason: Exclude<Turn['endReason'], 'error'>,
  at: number,
): Turn {
  return {
    id: nextId('turn'),
    userInput,
    steps: [],
    endReason: reason,
    createdAt: at,
    endedAt: at + 1_000,
  };
}

function completedTurn(
  userInput: string,
  answer: string,
  at: number,
  steps?: Step[],
): Turn {
  return {
    id: nextId('turn'),
    userInput,
    steps: steps ?? [
      { id: nextId('step'), assistantText: answer, toolCalls: [] },
    ],
    endReason: 'completed',
    createdAt: at,
    endedAt: at + 2_000,
  };
}

// A payload past the rendering limit, so the transcript caps can be seen.
function longToolCall(): ToolCall {
  const output = Array.from(
    { length: 500 },
    (_, index) => `line ${index + 1}: ${'x'.repeat(40)}`,
  ).join('\n');
  const changed = Array.from(
    { length: 450 },
    (_, index) => `const value${index} = ${index};`,
  );
  return {
    id: nextId('call'),
    name: 'shell',
    title: 'Run the test suite',
    args: JSON.stringify({ command: 'cargo test --workspace' }),
    pending: false,
    result: JSON.stringify({ stdout: output, exit_code: 0 }),
    isError: false,
    diffs: [
      {
        path: 'src/long_file.ts',
        oldText: changed.map((line) => `// old ${line}`).join('\n'),
        newText: changed.join('\n'),
      },
    ],
  };
}

function session(
  title: string,
  turns: Turn[],
  minutesAgo: number,
  cwd = '/home/jov/Projects/SrudAgent',
): Session {
  const updatedAt = NOW - minutesAgo * MINUTE;
  return {
    id: nextId('session'),
    backendId: nextId('backend'),
    cwd,
    loaded: true,
    title,
    model: MODEL,
    thinking: THINKING,
    sandboxMode: DEFAULT_SANDBOX_MODE,
    turns,
    createdAt: updatedAt,
    updatedAt,
  };
}

export function mockSessions(): { sessions: Session[]; activeId: string } {
  const base = NOW - 20 * MINUTE;
  const providerErrors = session(
    'Provider errors',
    [
      failedTurn(
        'Summarize this repository.',
        {
          code: -32603,
          message: 'the turn ended with an internal error',
        },
        base,
      ),
      failedTurn(
        'Try asking another provider.',
        { code: 401, message: 'invalid api key' },
        base + 2 * MINUTE,
      ),
      failedTurn(
        'Keep going.',
        { code: 429, message: 'rate limit exceeded' },
        base + 4 * MINUTE,
      ),
      failedTurn(
        'Retry once the provider is back.',
        { code: 503, message: 'service temporarily unavailable' },
        base + 6 * MINUTE,
      ),
      failedTurn(
        'Read the whole codebase into this turn.',
        { code: 400, message: 'maximum context length exceeded' },
        base + 8 * MINUTE,
      ),
      failedTurn(
        'Ask again over a flaky network.',
        { message: 'network timeout' },
        base + 10 * MINUTE,
      ),
      failedTurn(
        'Show a failure that kept no error text.',
        undefined,
        base + 12 * MINUTE,
      ),
    ],
    3,
  );

  const replayStates = session(
    'Replay states',
    [
      endedTurn('Stop this turn.', 'interrupted', base + 14 * MINUTE),
      endedTurn('Blocked by the sandbox.', 'blocked', base + 16 * MINUTE),
      failedTurn(
        'Failed after saying something.',
        { code: -32603, message: 'the turn ended with an internal error' },
        base + 18 * MINUTE,
        'The answer was cut off here.',
      ),
    ],
    9,
  );

  const longContent = session(
    'Long content',
    [
      completedTurn(
        'Show a long tool call and diff.',
        'Here is the result.',
        base + 20 * MINUTE,
        [
          {
            id: nextId('step'),
            thought:
              'I should inspect the test output first, then compare the changed file.\n' +
              'The diff is long, so the card should scroll instead of growing the page.',
            assistantText: 'Running the tests and comparing the file.',
            toolCalls: [longToolCall()],
          },
        ],
      ),
    ],
    20,
  );

  const normal = session(
    'Normal chat',
    [
      completedTurn('Add a retry to the HTTP client.', '', base + 22 * MINUTE, [
        {
          id: nextId('step'),
          thought:
            'I should look at how the request is made before changing it.\n' +
            'The wrapper is small, so one retry loop around the fetch should be enough.',
          assistantText: 'Checking the current client first.',
          toolCalls: [
            {
              id: nextId('call'),
              name: 'read',
              title: 'Read src/http.ts',
              args: JSON.stringify({ path: 'src/http.ts' }),
              pending: false,
              result: JSON.stringify({
                path: 'src/http.ts',
                lines: 42,
                content:
                  'export async function request(url: string) {\n' +
                  '  const response = await fetch(url);\n' +
                  '  if (!response.ok) throw new Error(`HTTP ${response.status}`);\n' +
                  '  return response.json();\n}',
              }),
            },
          ],
        },
        {
          id: nextId('step'),
          thought:
            'A retry belongs around the fetch, not inside the timeout helper.\n' +
            'Two attempts with a short delay keeps the change small.',
          assistantText: 'Adding a small retry loop around the request.',
          toolCalls: [
            {
              id: nextId('call'),
              name: 'edit',
              title: 'Edit src/http.ts',
              args: JSON.stringify({ path: 'src/http.ts' }),
              pending: false,
              result: 'ok',
              diffs: [
                {
                  path: 'src/http.ts',
                  oldText:
                    'export async function request(url: string) {\n' +
                    '  const response = await fetch(url);\n' +
                    '  if (!response.ok) throw new Error(`HTTP ${response.status}`);\n' +
                    '  return response.json();\n}',
                  newText:
                    'export async function request(url: string) {\n' +
                    '  for (let attempt = 0; attempt < 2; attempt += 1) {\n' +
                    '    const response = await fetch(url);\n' +
                    '    if (response.ok) return response.json();\n' +
                    '    if (attempt === 0) await sleep(250);\n' +
                    '  }\n' +
                    '  throw new Error(`HTTP request failed: ${url}`);\n}',
                },
              ],
            },
          ],
        },
        {
          id: nextId('step'),
          assistantText:
            'Done. The request now makes two attempts with a 250ms pause before the first retry.',
          toolCalls: [],
        },
      ]),
      completedTurn('Run the tests.', '', base + 24 * MINUTE, [
        {
          id: nextId('step'),
          thought:
            'The retry path is covered by the timeout test, so the existing suite should catch a mistake.',
          assistantText: 'Running the workspace tests.',
          toolCalls: [
            {
              id: nextId('call'),
              name: 'shell',
              title: 'Run cargo test',
              args: JSON.stringify({ command: 'cargo test --workspace' }),
              pending: false,
              result: JSON.stringify({
                stdout:
                  'running 42 tests\n' +
                  'test result: ok. 42 passed; 0 failed\n' +
                  'finished in 3.2s',
                stderr: '',
                exit_code: 0,
              }),
              isError: false,
            },
          ],
        },
        {
          id: nextId('step'),
          assistantText: 'All 42 tests pass, including the timeout retry case.',
          toolCalls: [],
        },
      ]),
      completedTurn('Also run lint.', '', base + 26 * MINUTE, [
        {
          id: nextId('step'),
          thought:
            'Lint is separate from tests; run it before claiming the change is clean.',
          assistantText: 'Checking the lints.',
          toolCalls: [
            {
              id: nextId('call'),
              name: 'shell',
              title: 'Run cargo clippy',
              args: JSON.stringify({ command: 'cargo clippy -- -D warnings' }),
              pending: false,
              result:
                '[tool error] clippy exited 1\n\n' +
                'warning: unused import `std::time::Duration` in src/http.rs:2',
              isError: true,
            },
          ],
        },
        {
          id: nextId('step'),
          thought:
            'The import came in with the sleep helper but the helper takes milliseconds.\n' +
            'Removing it is the smallest fix.',
          assistantText: 'One unused import; removing it.',
          toolCalls: [
            {
              id: nextId('call'),
              name: 'edit',
              title: 'Edit src/http.rs',
              args: JSON.stringify({ path: 'src/http.rs' }),
              pending: false,
              result: 'ok',
              diffs: [
                {
                  path: 'src/http.rs',
                  oldText: 'use std::time::Duration;\n',
                  newText: '',
                },
              ],
            },
          ],
        },
        {
          id: nextId('step'),
          assistantText: 'Lint is clean after removing the unused import.',
          toolCalls: [],
        },
      ]),
      completedTurn('Show me the final diff.', '', base + 28 * MINUTE, [
        {
          id: nextId('step'),
          thought:
            'The reader asked for the final state, not the intermediate edits.\n' +
            'Show the retry loop and the cleaned import.',
          assistantText: 'Here is the current state of the file.',
          toolCalls: [
            {
              id: nextId('call'),
              name: 'write',
              title: 'Write src/http.ts',
              args: JSON.stringify({ path: 'src/http.ts' }),
              pending: false,
              result: JSON.stringify({ path: 'src/http.ts', bytes: 214 }),
              diffs: [
                {
                  path: 'src/http.ts',
                  oldText:
                    'export async function request(url: string) {\n  const response = await fetch(url);\n}',
                  newText:
                    'export async function request(url: string) {\n' +
                    '  for (let attempt = 0; attempt < 2; attempt += 1) {\n' +
                    '    const response = await fetch(url);\n' +
                    '    if (response.ok) return response.json();\n' +
                    '    if (attempt === 0) await sleep(250);\n' +
                    '  }\n' +
                    '  throw new Error(`HTTP request failed: ${url}`);\n}',
                },
              ],
            },
          ],
        },
        {
          id: nextId('step'),
          assistantText:
            'That is the whole change: retry loop plus the unused import removal.',
          toolCalls: [],
        },
      ]),
    ],
    30,
  );

  const sessions = [providerErrors, replayStates, longContent, normal];
  return { sessions, activeId: sessions[0].id };
}
