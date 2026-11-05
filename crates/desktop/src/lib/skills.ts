import { rpcRequest } from './acp';

/** One skill as the agent reports it to a client. */
export interface SkillInfo {
  name: string;
  description: string;
  path: string;
  modelInvocable: boolean;
  userInvocable: boolean;
}

/** One skill's frontmatter, instructions and bundled files. */
export interface SkillDetail extends SkillInfo {
  license?: string;
  compatibility?: string;
  allowedTools: string[];
  metadata: Record<string, unknown>;
  body: string;
  supportingFiles: string[];
}

/**
 * Lists the skills visible from a working directory.
 *
 * An empty `cwd` asks for the user-wide library only, which is what the
 * sidebar shows before a session has a workspace.
 */
export async function listSkills(cwd: string): Promise<SkillInfo[]> {
  const result = await rpcRequest('_srud/unstable/skills/list', { cwd });
  return Array.isArray(result.skills) ? (result.skills as SkillInfo[]) : [];
}

/** Reads one skill, including its instructions and bundled files. */
export async function readSkill(
  cwd: string,
  name: string,
): Promise<SkillDetail> {
  const result = await rpcRequest('_srud/unstable/skills/read', { cwd, name });
  const skill = (result.skill ?? {}) as Partial<SkillDetail>;
  // The agent omits empty optional fields; normalize them so the pane can
  // render a skill without guarding every list it touches.
  return {
    name: skill.name ?? '',
    description: skill.description ?? '',
    path: skill.path ?? '',
    modelInvocable: skill.modelInvocable ?? true,
    userInvocable: skill.userInvocable ?? true,
    license: skill.license,
    compatibility: skill.compatibility,
    allowedTools: skill.allowedTools ?? [],
    metadata: skill.metadata ?? {},
    body: skill.body ?? '',
    supportingFiles: skill.supportingFiles ?? [],
  };
}
