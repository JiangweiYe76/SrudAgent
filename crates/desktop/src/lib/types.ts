// Domain model: session -> turn -> step.

export type TurnEndReason = 'completed' | 'interrupted' | 'blocked' | 'error';

// One tool invocation issued by a step's model response.
export interface ToolCall {
  id: string;
  name: string;
  args: string;
  result?: string;
}

// One model sampling request-response pair, plus the tool calls it issued.
export interface Step {
  id: string;
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
  title: string;
  turns: Turn[];
  createdAt: number;
  updatedAt: number;
}
