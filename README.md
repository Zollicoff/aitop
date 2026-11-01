# aitop

**A btop-style terminal dashboard for AI coding tool usage.** See what Claude Code, Codex CLI and Gemini CLI are costing you, per project, model and session, live, without API keys and without sending anything anywhere.

![aitop](docs/screenshot.png)

<sub>Screenshot generated from `scripts/make-demo-home.py` demo data.</sub>

## Features

- **Three tools, one screen.** Claude Code, OpenAI Codex CLI and Google Gemini CLI, each in its own panel, with totals in the header.
- **Live.** Logs are re-read incrementally every 2 s, so a running session shows up as it happens (a 500 MB history scans in under a second, rescans take milliseconds).
- **Costs that match the models you use.** A built-in price table covering Claude 3 → 5.x, GPT-5.x / Codex and Gemini 2 → 3.x, including cache reads, 5-minute and 1-hour cache writes, long-context tiers and Claude fast mode. Refresh it any time with `aitop pricing update`.
- **Projects, models, sessions.** Usage is grouped by git repository (subdirectories and worktrees are folded into their repo), by model, or by session, sortable by cost, tokens, recency or name.
- **Limits.** Claude's current 5-hour window with burn rate and projected spend; Codex's 5-hour and weekly rate-limit usage straight from its logs.
- **History that outlives the logs.** Claude Code deletes transcripts after 30 days; aitop keeps a compact copy of every usage record in `~/.local/share/aitop/history.jsonl`, so "all time" really means all time.
- **Scriptable.** `aitop report` prints the same numbers as a table or JSON.

## Install

From source (needs Rust 1.86+):

```bash
cargo install --git https://github.com/Zollicoff/aitop
```

Or clone and build:

```bash
git clone https://github.com/Zollicoff/aitop && cd aitop
cargo build --release
./target/release/aitop
```

Works on Linux and macOS.

## Usage

```bash
aitop                      # dashboard
aitop --range week         # start on the 7-day view (today | week | month | all)
aitop report               # usage per project, today
aitop report --range month --by model
aitop report --by day --tool claude --json
aitop pricing              # every model in your logs and the price applied to it
aitop pricing update       # download current prices
aitop doctor               # where aitop looks, what it found
aitop config > ~/.config/aitop/config.toml   # start a config file
```

### Keys

| Key | Action |
| --- | --- |
| `1` `2` `3` `4` / `[` `]` | range: today · 7 days · 30 days · all time |
| `tab` / `shift-tab` | focus next / previous tool |
| `↑` `↓` / `j` `k`, `PgUp` `PgDn`, `g` `G` | move the selection |
| `←` `→` / `h` `l` (or `p` `m` `S`) | projects · models · sessions |
| `s` | sort by cost · tokens · recent · name |
| `z` / `enter` | zoom the focused tool, with details for the selected row |
| `r` | rescan now |
| `?` | help, data locations and pricing info |
| `q` | quit |

The mouse works too: click tabs and ranges, scroll tables.

## Where the data comes from

Everything is read from files the tools already write on your machine:

| Tool | Location | What's in it |
| --- | --- | --- |
| Claude Code | `~/.claude/projects/**/*.jsonl` (also `~/.config/claude/projects`, or `$CLAUDE_CONFIG_DIR`) | per-response token usage, model, cwd, session; sub-agent transcripts included |
| Codex CLI | `~/.codex/sessions/**/rollout-*.jsonl` and `archived_sessions` (or `$CODEX_HOME`) | cumulative token counts per session, model, cwd, rate-limit snapshots |
| Gemini CLI | `~/.gemini/tmp/*/chats/session-*.json` | per-response token counts and model |

aitop never sends your data anywhere. The only network request it can make is `aitop pricing update`, which downloads the public [LiteLLM price list](https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json).

### How costs are computed

Costs are **estimates at API list prices**. If you use a Claude Pro/Max or ChatGPT subscription, they show what the same usage would cost on the API, not what you are billed. For each response:

```
cost = input × input_price + output × output_price
     + cache_read × cache_read_price
     + cache_write_5m × cache_write_price + cache_write_1h × cache_write_1h_price
```

- Reasoning/thinking tokens are part of output and billed as output.
- Codex reports cumulative totals; aitop bills the difference between consecutive snapshots and does not double-count resumed sessions.
- Claude streams a response as several log lines repeating the same usage; these are de-duplicated on message and request id.
- Prompts over 200K tokens use long-context prices where the model has them; Claude fast mode applies its premium.
- Models without a known price are flagged (`?` / `*` in tables, a warning in the footer). Add them under `[pricing]` in the config, or run `aitop pricing update`.

## Configuration

`~/.config/aitop/config.toml` (all optional; `aitop config` prints a commented example):

```toml
refresh_seconds = 2.0
default_range = "today"     # today | week | month | all
history = true              # keep ~/.local/share/aitop/history.jsonl
active_minutes = 5          # "active" = produced output in the last N minutes
hide_missing = false        # hide panels for tools that aren't installed

[paths]                     # override log locations
claude = ["~/.claude/projects"]

[pricing."my-model"]        # USD per million tokens
input = 1.0
output = 4.0
cache_read = 0.1
```

`NO_COLOR` disables colour; truecolor is used when the terminal advertises it, otherwise the 256-colour palette.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
python3 scripts/make-demo-home.py /tmp/aitop-demo
HOME=/tmp/aitop-demo cargo run -- --no-history
```

The code is small and split by concern: `sources/` (one incremental log reader per tool), `collector.rs` (merging, de-duplication, pricing, history), `aggregate.rs` (ranges, grouping, 5-hour windows), `ui/` (the dashboard) and `report.rs`.

## Credits

The look is inspired by [btop++](https://github.com/aristocratos/btop). aitop started as a fork of it and is now an independent Rust project sharing no code.

## License

Apache-2.0
