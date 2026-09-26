# CLAUDE.md

Repo-wide guidance for Claude Code. App-specific build/architecture details live in each app's own `CLAUDE.md` (e.g. `pico_w_display/CLAUDE.md`).

## Agent skills

### Issue tracker

Issues live in GitHub Issues (ntwyman/Automata), using the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default label vocabulary (needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix). See `docs/agents/triage-labels.md`.

### Domain docs

Multi-context layout: `CONTEXT-MAP.md` at repo root, per-app `CONTEXT.md` + `docs/adr/` (e.g. `pico_w_display/`). See `docs/agents/domain.md`.
