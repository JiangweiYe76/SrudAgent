// Domain model: session -> turn -> step.

export type TurnEndReason = 'completed' | 'interrupted' | 'blocked' | 'error';

// One model a session may run on, as the agent offers it.
//
// `value` is the agent's own identifier for it — a `providerId/modelId`
// reference — and is what a switch sends back. `name` is only what to show.
export interface ModelChoice {
  value: string;
  name: string;
}

// The models one provider offers, under that provider's heading.
export interface ModelGroup {
  id: string;
  name: string;
  choices: ModelChoice[];
}

// The model selector a session was handed.
//
// `configId` is the id the agent gave this option. ACP reserves the category
// `model` but not the id, so switching has to name the option the agent
// advertised instead of assuming what it is called.
//
// `currentValue` is the reference the session runs on. It can name something no
// group offers — a provider deleted since the session chose it — and is reported
// as it is rather than replaced, so a session whose model has gone reads as
// exactly that instead of as some other model.
export interface ModelOptions {
  configId: string;
  currentValue: string;
  groups: ModelGroup[];
}

// The thinking levels a model declares, in the agent's canonical vocabulary.
export type ThinkingLevel =
  | 'off'
  | 'minimal'
  | 'low'
  | 'medium'
  | 'high'
  | 'xhigh'
  | 'max';

// Every canonical level, weakest to strongest. The agent sends them in this
// order; this is also the order a form offers them in.
export const THINKING_LEVELS: readonly ThinkingLevel[] = [
  'off',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
];

// What a model declares about thinking: the level it starts on and the levels
// it offers. Levels are keyed by canonical level and map to the value the
// endpoint receives.
export interface ThinkingConfig {
  default: ThinkingLevel;
  levels: Partial<Record<ThinkingLevel, string>>;
}

// The thinking selector a session was handed.
//
// `currentValue` is the canonical level the session sends, not the endpoint's
// wire value: the mapping belongs to the model that declared it.
export interface ThinkingOptions {
  configId: string;
  currentValue: string;
  choices: ModelChoice[];
}

// One file change a tool call made, as ACP's diff content block carries it:
// the text before and after, not a patch. Drawing the comparison is the
// renderer's job.
export interface ToolDiff {
  path: string;
  // Null for a file the call created, which has no text before it.
  oldText: string | null;
  newText: string;
}

// One tool invocation issued by a step's model response.
export interface ToolCall {
  id: string;
  name: string;
  /**
   * The agent's one-line description of the call ("Edit main.rs").
   *
   * Falls back to the name for a call the agent never titled. Rendered in the
   * call's row, where a human reads what happened without opening the JSON.
   */
  title?: string;
  args: string;
  /**
   * Whether the model is still writing this call's arguments.
   *
   * True from the moment the agent names the tool, which is before it has the
   * arguments, until the update that carries them. A call whose arguments are a
   * whole file is most of the wait, and without this the row appears only once
   * there is nothing left to wait for.
   */
  pending: boolean;
  /**
   * The file changes the call reports, when it reports any.
   *
   * Absent for a call that made none and for a tool that cannot say from its
   * arguments alone which text it replaced.
   */
  diffs?: ToolDiff[];
  result?: string;
  /**
   * Whether the tool ran and failed.
   *
   * Sent alongside the result rather than inferred from it, because the two
   * arrive in different shapes: a failure is prose, a success is JSON, and which
   * one arrived says nothing about which it was until it is parsed.
   */
  isError?: boolean;
}

// One model sampling request-response pair, plus the tool calls it issued.
export interface Step {
  id: string;
  // Reasoning streamed before the answer, when the model exposes it.
  thought?: string;
  assistantText: string;
  toolCalls: ToolCall[];
}

// An image attached to a message: pasted into the composer or restored from
// the log, kept as base64.
export interface Attachment {
  id: string;
  mime: string;
  data: string;
}

// One user input and the full agent processing it triggers.
export interface Turn {
  id: string;
  userInput: string;
  // Images sent with the text above, in the order they were attached.
  images?: Attachment[];
  // Steered messages injected into this turn while it was running. Each is a
  // distinct user message and is rendered as its own bubble, in arrival order,
  // after `userInput` — they are not merged into it.
  steeredInputs?: string[];
  // The agent's own id for this turn, once an update has named it. Set for a
  // turn opened live as soon as its first update arrives, and the only thing
  // that puts a replayed update in the turn it belongs to.
  backendTurnId?: string;
  steps: Step[];
  // Absent while the turn is still running.
  endReason?: TurnEndReason;
  createdAt: number;
  endedAt?: number;
}

// The token usage a session has reported, as the latest `usage` update and the
// running totals seen across the session's steps.
export interface UsageState {
  // Tokens currently in the model's context, from the newest report.
  used: number;
  // The configured context window, from the newest report.
  size: number;
  // Sums over every step that reported, for the detail view and the cache rate.
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number;
  cacheWriteInputTokens: number;
  reasoningOutputTokens: number;
}

export interface Session {
  id: string;
  // The backend session id, once the session exists on the agent. Null until
  // then: clicks alone create neither a backend session nor a workspace
  // directory.
  backendId: string | null;
  // The directory the session works in, as the agent reported it. Loadable
  // sessions need it because ACP has the client confirm it; empty for a draft,
  // which has no directory until the agent gives it one.
  cwd: string;
  // Whether the conversation is on screen. False for a session the agent listed
  // but this client has not loaded, so its turns are still to come.
  loaded: boolean;
  // The agent's title, from the first message or a user rename. Absent until
  // the agent announces one.
  title: string | null;
  // Which model this conversation runs on. Null for a session the agent offers
  // no choice for — a host that configured no provider — and for one created
  // before the agent listed anything.
  model: ModelOptions | null;
  // Which thinking level this conversation sends. Null when the selected model
  // declares no thinking control.
  thinking?: ThinkingOptions | null;
  turns: Turn[];
  // Token usage, once a step has reported any. Absent for a session whose
  // model has no configured window — the agent sends nothing to show.
  usage?: UsageState;
  // Follow-up messages queued while a turn is running.
  followups?: string[];
  createdAt: number;
  updatedAt: number;
}
