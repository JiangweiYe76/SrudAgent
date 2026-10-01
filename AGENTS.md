# AGENTS.md

## Development Conventions

### Branching

- `main` is protected — always branch from it and open a PR back
- Branch names: `feature/<what>`, `fix/<what>`, `docs/<what>`, `refactor/<what>`

### Commit Messages

[Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `refactor:`, `docs:`, `test:`

Include a body in most commits — explain what changed and why, not just the type.

All commit messages in English.

### Code Style

- Format: `cargo fmt`
- Lint: `cargo clippy -- -D warnings`
- Follow Rust API guidelines: https://rust-lang.github.io/api-guidelines/

### Comments

- All comments in English
- Use `///` for public API docs
- Use `//` for inline explanations
- Avoid obvious comments — let code speak for itself
- Comments explain what the code does and why; never decision history, dates, or roadmap ("reserved for future...", "the plan was...")

### Git Safety

- Never commit unless explicitly asked
- Never push unless explicitly asked
- Always show the commit message to the user for review before committing
