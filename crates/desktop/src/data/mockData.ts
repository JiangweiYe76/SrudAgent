import type { Session } from '../lib/types';

// Seed data so the layout can be reviewed before the backend is wired up.
// Covers: markdown basics, GFM tables, code blocks, multi-step turns with
// tool calls, running turns, and non-completed end reasons.

let nextId = 100;
const id = (prefix: string) => `${prefix}${nextId++}`;

const MIN = 60 * 1000;
const now = Date.now();

export const seedSessions: Session[] = [
  {
    id: 's1',
    backendId: 's1',
    title: 'Multi-agent topology discussion',
    createdAt: now - 30 * MIN,
    updatedAt: Date.now() - 1000 * 60 * 5,
    turns: [
      {
        id: id('t'),
        userInput: 'When is multi-agent actually better than a single agent?',
        endReason: 'completed',
        createdAt: now - 12 * MIN,
        endedAt: now - 12 * MIN + 43 * 1000,
        steps: [
          {
            id: id('st'),
            assistantText:
              'Core conclusion: multi-agent essentially buys parallelism with tokens. It only pays off when the task splits into **independent** tracks.\n\n| Scenario | Multi-agent vs single |\n| --- | --- |\n| Parallelizable research | +80.8% |\n| Strongly sequential coding | -70% |\n| Mixed with clear subtasks | roughly break-even |\n\nSo default to a single agent first, and only split when the dependency graph allows it.',
            toolCalls: [],
          },
        ],
      },
      {
        id: id('t'),
        userInput: 'Which topology should SrudAgent start with?',
        endReason: 'completed',
        createdAt: now - 7 * MIN,
        endedAt: now - 7 * MIN + 2 * MIN + 17 * 1000,
        steps: [
          {
            id: id('st'),
            assistantText: 'Let me check the local research notes for existing conclusions.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'read_file',
                args: 'notes/multi-agent/README.md',
                result:
                  '# Multi-agent survey\n\n## Key conclusions\n\n1. Tokens buy parallelism; only pays off on independent tracks.\n2. Orchestrator + subagents is the recommended Tier 1 topology.\n3. Lead agent should dispatch 3-5 subagents at a time (Anthropic practice).\n...',
              },
            ],
          },
          {
            id: id('st'),
            assistantText:
              'Start with **orchestrator + subagents**: one main agent plans and dispatches, subagents handle independent subtasks and return results.\n\nConcretely:\n\n```rust\npub struct LeadAgent {\n    planner: Planner,\n    dispatcher: Dispatcher,\n}\n\nimpl LeadAgent {\n    pub async fn run(&self, task: Task) -> Result<Report> {\n        let subtasks = self.planner.decompose(task).await?;\n        let results = self.dispatcher.run_in_parallel(subtasks).await?;\n        Report::synthesize(results)\n    }\n}\n```\n\nConsider peer topologies later, once context isolation and memory schemes are settled.',
            toolCalls: [],
          },
        ],
      },
      {
        id: id('t'),
        userInput: 'Draft a one-page summary of the five core conclusions for the team.',
        // No endReason: this turn is still "running" — shows the loading state.
        createdAt: now - 5 * MIN,
        steps: [
          {
            id: id('st'),
            assistantText:
              'Pulling the conclusions together from the survey README. First pass at the outline:\n\n1. Tokens buy parallelism\n2. Independence is the precondition\n3. Orchestrator + subagents as the starting topology\n\nLet me verify the remaining two against the failure-modes doc.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'grep',
                args: 'pattern="failure" path=notes/multi-agent/05-failure-modes.md',
                result: 'notes/multi-agent/05-failure-modes.md:112: effort tiering\n...:118: interleaved thinking\n...:191: Claude 4 as prompt engineer',
              },
            ],
          },
        ],
      },
    ],
  },
  {
    id: 's2',
    backendId: 's2',
    title: 'Rust workspace layout',
    createdAt: now - 5 * 60 * MIN,
    updatedAt: Date.now() - 1000 * 60 * 60 * 3,
    turns: [
      {
        id: id('t'),
        userInput: 'Is putting Cargo.toml at the root correct?',
        endReason: 'completed',
        createdAt: now - 4 * 60 * MIN,
        endedAt: now - 4 * 60 * MIN + 38 * 1000,
        steps: [
          {
            id: id('st'),
            assistantText:
              'Yes, that is the standard Cargo workspace layout. The root Cargo.toml only holds `[workspace]` and members; Cargo.lock is shared across the whole workspace.\n\n> Keep shared metadata (version, edition) in `[workspace.package]` and inherit it with `workspace = true` in each member.',
            toolCalls: [],
          },
        ],
      },
      {
        id: id('t'),
        userInput: 'Run cargo metadata to verify the workspace resolves.',
        endReason: 'completed',
        createdAt: now - 3 * 60 * MIN,
        endedAt: now - 3 * 60 * MIN + 55 * 1000,
        steps: [
          {
            id: id('st'),
            assistantText: 'Running it now.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'cargo metadata --format-version 1',
                result: '{"packages":[...],"workspace_members":["srud-agent-desktop","core","app-server"],"resolve":{...}}',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'The workspace resolves cleanly: 3 members, no version conflicts.',
            toolCalls: [],
          },
        ],
      },
    ],
  },
  {
    id: 's3',
    backendId: 's3',
    title: 'Context & memory scheme',
    createdAt: now - 27 * 60 * MIN,
    updatedAt: Date.now() - 1000 * 60 * 60 * 26,
    turns: [
      {
        id: id('t'),
        userInput: 'Which layer should compaction live in?',
        endReason: 'completed',
        createdAt: now - 26 * 60 * MIN,
        endedAt: now - 26 * 60 * MIN + 29 * 1000,
        steps: [
          {
            id: id('st'),
            assistantText:
              "Put it in the runtime core layer: compact each agent's private context independently, and route shared memory through the artifact pattern to avoid duplication.",
            toolCalls: [],
          },
        ],
      },
      {
        id: id('t'),
        userInput: 'Estimate token savings of tool-result clearing on a typical research session.',
        endReason: 'completed',
        createdAt: now - 25 * 60 * MIN,
        endedAt: now - 25 * 60 * MIN + 1 * MIN + 5 * 1000,
        steps: [
          {
            id: id('st'),
            assistantText: 'Let me pull the numbers from the Anthropic research-system post.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'read_file',
                args: 'notes/multi-agent/04-context-and-memory.md',
                result: '...subagent explores tens of thousands of tokens, returns only a 1000-2000 token summary...',
              },
            ],
          },
          {
            id: id('st'),
            assistantText:
              'Rough estimate: a research session with 5 subagents accumulates ~150k tokens of raw tool output in the lead context. Clearing stale tool results keeps only the distilled summaries, cutting that to ~10k — about **90% savings** on the history portion.',
            toolCalls: [],
          },
        ],
      },
    ],
  },
  {
    id: 's4',
    backendId: 's4',
    title: 'Implement session/resume rollout replay',
    createdAt: now - 90 * MIN,
    updatedAt: now - 2 * MIN,
    turns: [
      {
        id: id('t'),
        userInput: 'Add session/resume: read the rollout JSONL and rebuild the in-memory session state.',
        endReason: 'completed',
        createdAt: now - 80 * MIN,
        endedAt: now - 71 * MIN,
        steps: [
          {
            id: id('st'),
            assistantText: 'Let me first look at how rollout files are currently written.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'grep',
                args: 'pattern="rollout" path=crates/core/src',
                result: 'crates/core/src/persist/mod.rs:14: pub fn append_item(...)\ncrates/core/src/persist/rollout.rs:8: pub struct RolloutWriter',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Found the writer. Now reading it to understand the record format.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'read_file',
                args: 'crates/core/src/persist/rollout.rs',
                result: 'pub enum RolloutLine {\n    SessionMeta(SessionMeta),\n    ResponseItem(ResponseItem),\n    TurnComplete(TurnEndReason),\n}',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'The format is clear. I will add a reader with the same enum, then a `resume` function that replays lines into a `SessionState`.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'edit_file',
                args: 'crates/core/src/persist/rollout.rs (+72 lines)',
                result: 'ok',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Now wiring `session/resume` into the app-server dispatch table.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'read_file',
                args: 'crates/app-server/src/dispatch.rs',
                result: 'match method {\n    "session/start" => ..., \n    "session/list" => ...,\n    _ => Err(MethodNotFound),\n}',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: '',
            toolCalls: [
              {
                id: id('tc'),
                name: 'edit_file',
                args: 'crates/app-server/src/dispatch.rs (+15 lines)',
                result: 'ok',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Compiling to check the new code.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'cargo build -p core -p app-server',
                result: 'error[E0382]: borrow of moved value: `lines`\n  --> crates/core/src/persist/replay.rs:41:9',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Borrow checker complaint — `lines` is moved into the loop. Fixing by iterating over a reference.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'edit_file',
                args: 'crates/core/src/persist/replay.rs (fix E0382)',
                result: 'ok',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: '',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'cargo build -p core -p app-server',
                result: 'Finished dev profile in 4.2s',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Adding a round-trip test: write a rollout, resume it, assert history matches.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'edit_file',
                args: 'crates/core/src/persist/replay.rs (+48 lines, test)',
                result: 'ok',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Running the persist test module.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'cargo test -p core persist::',
                result: 'test persist::replay::tests::round_trip ... ok\ntest result: ok. 3 passed; 0 failed',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'One more guard: a rollout ending mid-turn must close with a synthetic `Interrupted`.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'edit_file',
                args: 'crates/core/src/persist/replay.rs (+22 lines)',
                result: 'ok',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Final check — clippy must be clean before I call this done.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'cargo clippy -- -D warnings',
                result: 'warning: this can be `digest` ... ignored\nerror: manual implementation of `Iterator::find`: crates/core/src/persist/replay.rs:88',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: '',
            toolCalls: [
              {
                id: id('tc'),
                name: 'edit_file',
                args: 'crates/core/src/persist/replay.rs:88 (use .find())',
                result: 'ok',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Done. `session/resume` replays rollout JSONL into session state, mid-turn rollouts get a synthetic `Interrupted` close, and the round-trip test covers it. Clippy and tests are green.',
            toolCalls: [],
          },
        ],
      },
      {
        id: id('t'),
        userInput: 'Commit it.',
        endReason: 'completed',
        createdAt: now - 8 * MIN,
        endedAt: now - 7 * MIN,
        steps: [
          {
            id: id('st'),
            assistantText: 'Staging the persist changes and committing with a conventional message.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'git add crates/core/src/persist crates/app-server/src/dispatch.rs',
                result: '(no output)',
              },
              {
                id: id('tc'),
                name: 'shell',
                args: 'git commit -m "feat: add session/resume rollout replay"',
                result: '[feature/session-resume 4f2a9c1] feat: add session/resume rollout replay\n 3 files changed, 157 insertions(+)',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Committed as `4f2a9c1`. Not pushed — let me know when you want the PR.',
            toolCalls: [],
          },
        ],
      },
      {
        id: id('t'),
        userInput: 'Push and open the PR against main.',
        endReason: 'completed',
        createdAt: now - 6 * MIN,
        endedAt: now - 5 * MIN,
        steps: [
          {
            id: id('st'),
            assistantText: 'Pushing the branch and creating the PR with a summary of the rollout replay design.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'git push -u origin feature/session-resume',
                result: 'branch \'feature/session-resume\' set up to track \'origin/feature/session-resume\'.',
              },
              {
                id: id('tc'),
                name: 'shell',
                args: 'gh pr create --title "feat: add session/resume rollout replay" --body "..."',
                result: 'https://github.com/srud/SrudAgent/pull/7',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'PR #7 is up: **feat: add session/resume rollout replay**. CI is running.',
            toolCalls: [],
          },
        ],
      },
      {
        id: id('t'),
        userInput: 'CI failed on clippy. Fix it.',
        endReason: 'completed',
        createdAt: now - 4 * MIN,
        endedAt: now - 2 * MIN,
        steps: [
          {
            id: id('st'),
            assistantText: 'Pulling the failing check log.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'gh run view --log-failed',
                result: 'error: unused variable: `reason`\n  --> crates/core/src/persist/replay.rs:96:13',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Local clippy passed because of a stale cache. Prefixing the variable to silence it properly.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'edit_file',
                args: 'crates/core/src/persist/replay.rs:96 (rename `reason` -> `_reason`)',
                result: 'ok',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: '',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'cargo clean -p core && cargo clippy -- -D warnings',
                result: 'Finished dev profile in 31.4s',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: '',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'git commit -am "fix: silence unused variable in replay" && git push',
                result: 'To github.com:srud/SrudAgent.git\n   4f2a9c1..9be3d70  feature/session-resume',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Pushed `9be3d70`. Clippy is clean with a fresh cache this time; CI should go green.',
            toolCalls: [],
          },
        ],
      },
      {
        id: id('t'),
        userInput: 'Why do we synthesize an Interrupted close instead of leaving the turn open?',
        endReason: 'completed',
        createdAt: now - 90 * 1000,
        endedAt: now - 60 * 1000,
        steps: [
          {
            id: id('st'),
            assistantText:
              'Because the UI derives its loading state from **exactly one** terminal event per turn (04-core-concepts §3 invariant 1). A rollout that stops mid-turn has no real `turnComplete` line; without the synthetic close, `session/resume` would rebuild a session whose last turn looks perpetually running, and every reconnect would re-enter a loading state that can never finish. Writing `Interrupted` at replay time makes the recovered history self-consistent — same trick dsh uses with `interruptedTurnClosers`.',
            toolCalls: [],
          },
        ],
      },
      {
        id: id('t'),
        userInput: 'Also add a fuzz test for malformed rollout lines.',
        endReason: 'completed',
        createdAt: now - 2 * MIN,
        endedAt: now - 40 * 1000,
        steps: [
          {
            id: id('st'),
            assistantText: 'Checking whether the workspace already has a fuzzing setup.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'ls crates/core/fuzz 2>/dev/null; grep -r arbitrary Cargo.toml',
                result: '(no fuzz dir, no arbitrary dep yet)',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'No fuzz target exists yet, so I will add a minimal one that feeds arbitrary bytes into the rollout line parser.',
            toolCalls: [
              {
                id: id('tc'),
                name: 'edit_file',
                args: 'crates/core/fuzz/fuzz_targets/rollout_parse.rs (+31 lines)',
                result: 'ok',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: '',
            toolCalls: [
              {
                id: id('tc'),
                name: 'shell',
                args: 'cargo fuzz run rollout_parse -- -max_total_time=30',
                result: 'info: corpus: 212 files; execs: 41k; no crashes found',
              },
            ],
          },
          {
            id: id('st'),
            assistantText: 'Fuzz target added and 30s run found no crashes — the parser rejects malformed lines cleanly. Committed as `c1d0e9f`.',
            toolCalls: [],
          },
        ],
      },
    ],
  },
];
