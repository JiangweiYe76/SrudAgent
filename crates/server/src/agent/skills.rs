//! Skill discovery over `_srud/unstable/skills/*`.
//!
//! `list` exposes the same roots and validation the turn loop uses as names,
//! descriptions and paths, plus the names it shadowed. `read` adds one skill's
//! frontmatter, instructions and bundled files, which a client asks for only
//! after opening a skill.

use std::path::PathBuf;

use serde_json::Value;
use srud_protocol::acp::AcpError;
use srud_protocol::error::{INTERNAL_ERROR, INVALID_PARAMS};
use srud_protocol::srud::methods::{
    ListSkillsRequest, ListSkillsResponse, ReadSkillRequest, ReadSkillResponse, SkillConflictInfo,
    SkillDetail, SkillInfo,
};

use super::{parse_params, serialize, Agent};

impl Agent {
    /// Lists every discovered skill and shadowed name for a working directory.
    pub(super) fn handle_skills_list(&self, params: Value) -> Result<Value, AcpError> {
        let request: ListSkillsRequest = parse_params(params)?;
        let cwd = PathBuf::from(request.cwd);
        // A client that has no workspace sends an empty path. Anything relative
        // is treated the same way: resolving it against the server's cwd would
        // advertise a project root the user did not choose.
        let cwd = if cwd.is_absolute() {
            cwd
        } else {
            PathBuf::new()
        };
        let roots = srud_core::skills::roots(&cwd);
        let discovery = srud_core::skills::discover_with_conflicts(&roots);
        Ok(serialize(ListSkillsResponse {
            skills: discovery
                .skills
                .into_iter()
                .map(|skill| SkillInfo {
                    name: skill.name,
                    description: skill.description,
                    path: skill.path.display().to_string(),
                    model_invocable: skill.model_invocable,
                    user_invocable: skill.user_invocable,
                })
                .collect(),
            conflicts: discovery
                .conflicts
                .into_iter()
                .map(|conflict| SkillConflictInfo {
                    name: conflict.name,
                    winner_path: conflict.winner.display().to_string(),
                    loser_path: conflict.loser.display().to_string(),
                })
                .collect(),
            meta: None,
        }))
    }

    /// Reads one skill's metadata and instructions.
    pub(super) fn handle_skill_read(&self, params: Value) -> Result<Value, AcpError> {
        let request: ReadSkillRequest = parse_params(params)?;
        let cwd = PathBuf::from(request.cwd);
        let cwd = if cwd.is_absolute() {
            cwd
        } else {
            PathBuf::new()
        };
        let roots = srud_core::skills::roots(&cwd);
        let found = srud_core::skills::discover(&roots);
        let Some(skill) = found.into_iter().find(|skill| skill.name == request.name) else {
            return Err(AcpError::new(
                INVALID_PARAMS,
                format!("no skill named `{}` in this workspace", request.name),
            ));
        };

        let text = std::fs::read_to_string(&skill.path).map_err(|err| {
            AcpError::new(
                INTERNAL_ERROR,
                format!("cannot read {}: {err}", skill.path.display()),
            )
        })?;

        Ok(serialize(ReadSkillResponse {
            skill: SkillDetail {
                name: skill.name,
                description: skill.description,
                path: skill.path.display().to_string(),
                model_invocable: skill.model_invocable,
                user_invocable: skill.user_invocable,
                license: skill.license,
                compatibility: skill.compatibility,
                allowed_tools: skill.allowed_tools,
                metadata: skill.metadata,
                body: srud_core::skills::body(&text).to_owned(),
                supporting_files: srud_core::skills::supporting_files(&skill.path),
            },
            meta: None,
        }))
    }
}
