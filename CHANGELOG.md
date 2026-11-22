# Changelog

## Unreleased

### Changed

- Internal clean-up, with no change in behaviour: the collector, the
  Codex reader, the T3 Code client and the Options menu are each split
  into smaller modules; unused fields and serialization code are removed;
  the README's code map and settings reference are brought up to date.
- `scripts/make-demo-home.py` writes Codex logs in the form Codex itself
  does, so the demo data shows Codex usage again. It also writes what
  T3 Code would report (other agents and a linked machine), and the
  README's screenshots are new.

### Fixed

- Codex sessions that Codex has compressed (`rollout-*.jsonl.zst`) were
  not read at all, so older Codex usage was missing. They are read now.
  Each is decompressed once and its records kept in
  `~/.cache/aitop/codex-zst`, so later starts stay fast.

## [2.4.0] - 2026-11-22

### Added

- A second layout: the agents in a list with the selected one in full
  beside it. `layout = "auto" | "list" | "stacked"` (auto: stacked for one
  or two agents, the list for more), `v` to switch, and Options → Display
  → Layout.
- Options → Agents: an on/off switch for each agent, saved as `[agents]`
  in the config. One that is off is left out of the panels, the header
  totals and the activity feed.

### Changed

- Each agent T3 Code tracks (Grok Build, Cursor, OpenCode, Antigravity,
  Pi, Muse Code) has a panel of its own, under its own name, with Models
  and Machines tabs, instead of sharing "Other agents". That panel remains
  only for agents this aitop does not know by name. `--tool` takes the new
  names; records 2.3.0 saved under "other" are filed under their agent.
- `aitop report` leaves out an agent with no usage in the range.

## [2.3.0] - 2026-11-21

### Added

- Other agents: whatever else T3 Code tracks besides Claude Code and Codex
  (Grok Build, Cursor, Pi, OpenCode, Antigravity, Muse Code, …) appears in
  a third panel once there is usage of it, on this machine or a linked
  one, with Agents, Models and Machines tabs. `aitop report --tool other`
  prints it.

### Changed

- Codex logs are read the way T3 Code reads them: one record per request,
  checked against the session's running total. History that a forked
  session or sub-agent copies from its parent is no longer billed again.
  Sessions read this way replace their older records in the local
  history; sessions whose logs are gone keep what was recorded.
- Codex requests made at the fast ("priority") or ultrafast speed tier are
  charged that tier's prices, and long-context prices start at the
  threshold the price list gives (272K tokens for some models). Run
  `aitop pricing update` to get these tiers without T3 Code.

## [2.2.1] - 2026-11-21

### Changed

- Prices come from T3 Code when it is installed: aitop uses the price
  list T3 keeps on this machine (so new models are priced without
  `aitop pricing update`, and costs match T3's Usage page), and keeps the
  cost T3 reports for linked machines instead of re-pricing their daily
  totals. `[pricing]` overrides still win; `[t3] prices = false` turns
  this off. The first read after upgrading fetches each linked machine's
  history again to pick up its costs.

### Fixed

- A model missing from the price table could silently take the price of
  an older model with a similar name (for example `claude-fable-5-2`
  priced as `claude-fable-5`). With T3 Code's list it gets its own price.

## [2.2.0] - 2026-11-21

### Added

- T3 Code: when it is installed and running, aitop reads the transcript
  folders it uses on this machine (so T3's own config folders are counted
  without `[paths]`) and shows the machines linked in it, with their usage
  per tool, model and day. Nothing to set up: aitop gives itself read-only
  access with T3's own CLI, listed in T3 as `aitop`. `[t3] enabled = false`
  turns it off. Linked machines need T3 Code from 21 November 2026 or
  later on both sides.
- Several machines: list your other machines under `[[remote]]` and aitop
  pulls their usage over ssh (by running `aitop export` there) and shows
  it alongside this machine's. A Machines tab (`M`) and drill-down per
  panel, a machine column in the activity feed, `aitop report --by
  machine` / `--machine NAME`, and `aitop sync` to pull on demand.
  Remote usage is kept in the local history, so a machine that is asleep
  still shows what it last sent, and a response visible from two
  machines is counted once.
- `host = "…"` names this machine; `aitop doctor` shows it and the
  remotes.

### Changed

- History records from another machine carry its name (`h`), and daily
  totals read through T3 Code carry what they stand for (`n`, `ns`). Older
  history files load unchanged.
- New dependencies: `tungstenite` (T3 Code's WebSocket API), `gethostname`
  and `iana-time-zone`.

## [2.1.0] - 2026-11-04

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
