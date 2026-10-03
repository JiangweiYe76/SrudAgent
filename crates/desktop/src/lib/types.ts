// Domain model: session -> turn -> step.

export type TurnEndReason = 'completed' | 'interrupted' | 'blocked' | 'error';

// One tool invocation issued by a step's model response.
export interface ToolCall {
  id: string;
  name: string;
  args: string;
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

// One user input and the full agent processing it triggers.
export interface Turn {
  id: string;
  userInput: string;
  steps: Step[];
  // Absent while the turn is still running.
  endReason?: TurnEndReason;
  createdAt: number;
  endedAt?: number;
}

export interface Session {
  id: string;
  // The backend session id, once the draft has been materialized by the first
  // message. Null until then: clicks alone create no backend session and no
  // workspace directory.
  backendId: string | null;
  // The agent's title, from the first message or a user rename. Absent until
  // the agent announces one.
  title: string | null;
  turns: Turn[];
  createdAt: number;
  updatedAt: number;
}
