# Changelog

## [Unreleased]

### Added

- Themes: eight built in (aitop, catppuccin, dracula, gruvbox, nord,
  solarized-light, terminal, tokyo-night) and custom TOML themes in
  `~/.config/aitop/themes/`; `--theme`, `theme = "…"`, `t` / `T` to cycle,
  `aitop themes` and `aitop themes show NAME`.
- Budgets: daily / weekly / monthly spend limits overall or per tool
  (`[budget]`), with meters, projections, amber/red costs, footer alerts
  and an optional bell; also in `aitop report` and `aitop doctor`.
- Drill-down: Enter narrows a panel to a project, model or session
  (repeatable, Esc to back out). `/` searches the table.
- Live activity feed of the newest responses (`a`, or `feed = true`).
- `aitop statusline` for tmux, shell prompts and Claude Code's status
  line, with `--format` placeholders and `--color ansi|tmux`.
- A btop-style menu on Esc (Options · Help · Quit). The Options screen
  edits settings live and saves them to `config.toml`, keeping its
  comments and layout.
- Look settings: `theme_background` (off for transparent terminals),
  `truecolor` (auto / on / off), `box_style` (rounded / plain / double /
  thick), `graph_symbol` (braille / block / tty), `graph_color` (accent /
  gradient), `chart` (cost / tokens) and `clock_format`.

### Changed

- Enter now drills down; zoom is `z` only. Esc opens the menu once
  there is nothing left to back out of.
- The footer fits as many key hints as the width allows and no longer
  hides status messages on narrower terminals.

### Fixed

- Large initial scans were slow (quadratic de-duplication introduced in
  the 2.0.0 restructure): 25k responses now merge in about 0.16 s
  instead of 2.9 s.

## [2.0.0] - 2026-11-01

A ground-up rewrite. aitop is no longer a fork of btop++: it is an
independent Rust project (built on ratatui) that shares no code with it.
Versioning starts at 2.0.0 to mark the break from the earlier btop-based
prototype; btop's own v1.x tags have been removed from this repository.

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
