# AGENTS.md

## Development Conventions

### Branching

- `main` is protected — always branch from it and open a PR back
- Branch names: `<prefix>/<what>` in kebab-case: `feature/`, `fix/`, `docs/`, `refactor/`, `chore/`, `build/`, `style/`, `test/`, `perf/`

### Commit Messages

- Follow Conventional Commits, with a scope in most commits
- Include a body in most commits — explain what changed and why, not just the type
- Wrap body text into short lines instead of one long line
- All commit messages in English

### Code Style

- Format: `cargo fmt`
- Lint: `cargo clippy -- -D warnings`
- Front-end format: `pnpm format` in `crates/desktop`
- Follow Rust API guidelines: https://rust-lang.github.io/api-guidelines/

### Comments

- All comments in English
- Rust: `///` for public API docs, `//` for inline explanations
- TypeScript / TSX: `/** */` above the declaration it documents, `//` for inline explanations
- Avoid obvious comments — let code speak for itself
- Comments explain what the code does and why; never decision history, dates, or roadmap ("reserved for future...", "the plan was...")

### Git Safety

- Never commit unless explicitly asked
- Never push unless explicitly asked
- Always show the commit message to the user for review before committing
