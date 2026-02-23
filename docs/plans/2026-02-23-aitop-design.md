# aitop Design Document

## Overview

Fork of btop++ repurposed as a terminal-based AI coding tool usage monitor. Tracks usage for three CLI tools: Claude Code, Codex, and Gemini CLI. Displays token consumption, estimated costs, session activity, and per-repo breakdowns in a btop-style TUI.

## Layout

Three equal horizontal bands, one per tool. Each band is split vertically:
- **Left half:** Usage stats (total tokens, estimated cost, session count, active status)
- **Right half:** Scrollable repo list ranked by token usage/estimated cost

```
┌── Claude Code ──────────────────────────────────────────┐
│                          │                              │
│  Total tokens: 1.2M      │  REPO          TOKENS  COST │
│  Est. cost:   $4.82      │  empire        420K   $1.68 │
│  Sessions:    14          │  aitop         310K   $1.24 │
│  Active now:  Yes         │  SRPL          180K   $0.72 │
│                          │  ...    (scrollable)        │
├── Codex ────────────────────────────────────────────────┤
│                          │                              │
│  Total tokens: 800K      │  REPO          TOKENS  COST │
│  Est. cost:   $2.10      │  empire        500K   $1.31 │
│  Sessions:    8           │  data          300K   $0.79 │
│  Active now:  No          │  ...    (scrollable)        │
│                          │                              │
├── Gemini ───────────────────────────────────────────────┤
│                          │                              │
│  Total tokens: 500K      │  REPO          TOKENS  COST │
│  Est. cost:   $0.35      │  research      400K   $0.28 │
│  Sessions:    3           │  tensor_logic  100K   $0.07 │
│  Active now:  No          │  ...    (scrollable)        │
│                          │                              │
└─────────────────────────────────────────────────────────┘
```

## Approach

Fork btop++ (C++) and modify:
- **Keep:** Terminal rendering engine, box drawing, colors, Unicode, keyboard input, theme system, scrollable list widget, refresh/polling loop
- **Remove:** All system monitoring (CPU, memory, disk, network, process collection), platform-specific data collectors, existing layout definitions
- **Replace:** Data sources with AI tool log parsers, layout with three-band design

## Data Sources

All data is sourced from local log/config parsing. No API keys required.

### Claude Code
- **Log location:** `~/.claude/projects/**/*.jsonl`
- **Format:** JSONL with fields: `message.id`, `message.model`, `message.usage` (input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens), `sessionId`, `requestId`, `costUSD`, `cwd`
- **Session ID:** Extracted from directory path (`path.basename(path.dirname(filePath))`)
- **Project path:** Extracted from `cwd` field
- **Reference implementation:** `~/Github/Claude_Code_CLI_Usage/src/services/logParser.ts`

### Codex (OpenAI)
- **Log location:** TBD — investigate `~/.codex/` or similar
- **Format:** TBD — investigate local log structure

### Gemini CLI (Google)
- **Log location:** TBD — investigate `~/.gemini/` or similar
- **Format:** TBD — investigate local log structure

## Cost Estimation

Estimate costs from token counts using known per-token pricing. Rates stored in a configurable pricing table.

### Claude Code Pricing (per million tokens)

| Model | Input | Output | Cache Write | Cache Read |
|-------|-------|--------|-------------|-----------|
| Opus 4.5 | $5.00 | $25.00 | $6.25 | $0.50 |
| Opus 4.1 | $15.00 | $75.00 | $18.75 | $1.50 |
| Opus 4 | $15.00 | $75.00 | $18.75 | $1.50 |
| Sonnet 4.5 | $3.00 | $15.00 | $3.75 | $0.30 |
| Sonnet 4 | $3.00 | $15.00 | $3.75 | $0.30 |
| Haiku 4.5 | $1.00 | $5.00 | $1.25 | $0.10 |
| Haiku 3.5 | $0.80 | $4.00 | $1.00 | $0.08 |

### Codex / Gemini Pricing
TBD — will add once log formats are investigated.

### Calculation
```
cost = (input_tokens * input_rate + output_tokens * output_rate + cache_write_tokens * cache_write_rate + cache_read_tokens * cache_read_rate) / 1,000,000
```

## Caching

Port the caching strategy from Claude_Code_CLI_Usage:
- Cache location: `~/.aitop/cache/`
- Separate cache files per tool (claude.json, codex.json, gemini.json)
- Preserves historical data beyond each tool's log retention period
- Deduplication on merge using composite keys (timestamp + session + model + tokens)
- Cache refreshed on each polling cycle

## Keyboard Navigation

Reuse btop's input system:
- Arrow keys / vim keys to navigate between bands and scroll repo lists
- Tab to cycle focus between the three bands
- q to quit
- Theme switching (btop's existing theme system)

## Stats Displayed Per Tool (Left Half)

- Total tokens (input + output)
- Estimated total cost (USD)
- Total session count
- Active now indicator (is a session currently running)
- Model breakdown (which models used)

## Repo List Per Tool (Right Half)

Scrollable list, sorted by cost descending:
- Repository name
- Token count
- Estimated cost

## Tech Stack

- **Language:** C++ (inherited from btop)
- **Build:** CMake (btop's existing build system)
- **Dependencies:** btop's existing dependencies only
- **Reference code:** `~/Github/Claude_Code_CLI_Usage/` (TypeScript — port parsing logic to C++)
