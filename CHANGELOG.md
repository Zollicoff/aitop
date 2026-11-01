# Changelog

## [2.0.0] - 2026-11-01

A ground-up rewrite. aitop is no longer a fork of btop++: it is an
independent Rust project (built on ratatui) that shares no code with it.
Releases up to v1.4.6 belong to the earlier btop-based prototype.

### Added

- Dashboard for **Claude Code** and **Codex CLI** usage: cost, tokens
  (input, output, cache read/write), sessions and activity per tool, with
  today / 7-day / 30-day / all-time ranges and a cost-over-time chart.
- Tables by project (git repository, with worktrees folded in), model and
  session, sortable by cost, tokens, recency or name; zoom view with
  per-row details; mouse support.
- Claude's current 5-hour usage window with burn rate and projected cost;
  Codex 5-hour and weekly rate-limit usage from its logs.
- Built-in price table for Claude and OpenAI models, including cache
  reads, 5-minute and 1-hour cache writes, long-context tiers and Claude
  fast mode; `aitop pricing update` refreshes it from LiteLLM's public
  price list, and the config file can override prices.
- Local history (`~/.local/share/aitop/history.jsonl`) so totals survive
  the tools deleting old logs.
- `aitop report` (table or `--json`), `aitop pricing`, `aitop doctor`
  and `aitop config`.

### Changed

- Logs are parsed as JSON and read incrementally, so rescans are cheap
  even with large histories. Streamed Claude responses are de-duplicated,
  and Codex's cumulative counters are converted to per-turn usage without
  double-counting resumed sessions.

### Removed

- Gemini CLI support: Google retired Gemini CLI for individual accounts
  on 2026-06-18.
