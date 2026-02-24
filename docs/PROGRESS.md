# aitop — Project Progress

## What is aitop?

A fork of btop++ (C++ terminal system monitor) repurposed as an **AI coding tool usage monitor**. It shows usage stats for **Claude Code**, **Codex (OpenAI)**, and **Gemini CLI** in a btop-style TUI with three horizontal bands.

## Repository

- **GitHub:** https://github.com/Zollicoff/aitop
- **Branch:** `dev` (all work is here)
- **Forked from:** aristocratos/btop

## Layout Design

```
┌── Claude Code ──────────────────────────────────────────┐
│  [usage stats]          │  [scrollable repo list]       │
├── Codex ────────────────────────────────────────────────┤
│  [usage stats]          │  [scrollable repo list]       │
├── Gemini ───────────────────────────────────────────────┤
│  [usage stats]          │  [scrollable repo list]       │
└─────────────────────────────────────────────────────────┘
```

Each band: left half = total tokens, est. cost, sessions, active now. Right half = repos sorted by cost, scrollable.

## What's Been Done (Tasks 1-9 of 12)

### Commit History (dev branch)

| Commit | Description |
|--------|-------------|
| `d29e841` | Data structures (`src/aitop_shared.hpp`) — ToolType, RepoUsage, ToolStats, AitopData |
| `a9332a6` | Claude Code log parser (`src/aitop_claude.hpp/cpp`) — parses `~/.claude/projects/**/*.jsonl`, model-specific pricing, deduplication |
| `e60a6f4` | Codex log parser (`src/aitop_codex.hpp/cpp`) — parses `~/.codex/sessions/**/*.jsonl`, handles cumulative token counts |
| `7a868d7` | Gemini log parser (`src/aitop_gemini.hpp/cpp`) — parses `~/.gemini/tmp/*/chats/*.json`, uses actual token counts from session data |
| `085e27b` | Replaced btop system collectors with aitop parsers in CMakeLists.txt and Runner thread |
| `ae73004` | Three-band TUI layout (`src/aitop_draw.hpp/cpp`) — drawAll(), calcSizes(), selection(), BandState |
| `9e59d9e` | Wired drawing into Runner thread, replaced input handling with band navigation (tab/arrows), fixed GCC 13 ranges::to issue, created stubs for remaining btop symbols |
| `e66d3ea` | Removed all platform collectors (linux/osx/freebsd/openbsd/netbsd), stripped old draw functions, removed unused structs |

### Key Files Created

| File | Purpose |
|------|---------|
| `src/aitop_shared.hpp` | Data structures shared across all modules |
| `src/aitop_claude.hpp/cpp` | Claude Code log parser (315 lines) |
| `src/aitop_codex.hpp/cpp` | Codex CLI log parser (312 lines) |
| `src/aitop_gemini.hpp/cpp` | Gemini CLI log parser (413 lines) |
| `src/aitop_draw.hpp/cpp` | Three-band TUI rendering (316 lines) |
| `src/aitop_stubs.cpp` | Stub definitions for old btop symbols still referenced |
| `docs/plans/2026-02-23-aitop-design.md` | Design document |
| `docs/plans/2026-02-23-aitop-implementation.md` | Full implementation plan (12 tasks) |

### Build Status

The project **builds successfully** with GCC 13 on Linux (WSL2). Produces a binary. Has NOT been test-run yet.

### Data Sources Discovered

| Tool | Log Location | Format | Token Data |
|------|-------------|--------|------------|
| Claude Code | `~/.claude/projects/**/*.jsonl` | JSONL | Direct token counts + costUSD |
| Codex | `~/.codex/sessions/YYYY/MM-DD/*.jsonl` | JSONL | Cumulative `total_token_usage` per session |
| Gemini | `~/.gemini/tmp/*/chats/session-*.json` | JSON | `tokens` object with input/output/thoughts/cached |

## What's Left (Tasks 10-12)

### Task 10: Rename binary and branding
- Change CMake project name from `btop` to `aitop`
- Update version/title strings in `src/btop.cpp`
- Change config path from `~/.config/btop/` to `~/.config/aitop/`
- Rename conf file from `btop.conf` to `aitop.conf`

### Task 11: Test with real data and polish
- Run the binary and verify all three bands show real data
- Verify Claude band populates from `~/.claude/projects/`
- Verify Codex band populates from `~/.codex/sessions/`
- Verify Gemini band populates from `~/.gemini/tmp/`
- Test keyboard navigation (Tab between bands, arrows to scroll)
- Fix any rendering issues (column alignment, colors, borders)
- This will likely require iteration

### Task 12: Push to GitHub and README
- Push final changes
- Write a README.md explaining what aitop is, how to build, how to use
- Consider updating the repo description on GitHub

## How to Build

```bash
cd ~/Github/aitop
mkdir -p build && cd build
cmake .. -DBTOP_GPU=OFF
make -j$(nproc)
./btop  # still named btop until Task 10
```

## How to Continue

1. Open the repo: `cd ~/Github/aitop && git checkout dev`
2. Read the full plan: `docs/plans/2026-02-23-aitop-implementation.md`
3. Pick up from Task 10 (rename) — it's the next task
4. The implementation plan has exact code and commands for each remaining task

## Reference Repo

Your existing Claude Code tracking VS Code extension is at `~/Github/Claude_Code_CLI_Usage/`. The parsing logic in `src/services/logParser.ts` and `src/services/pricing.ts` was used as reference for the Claude parser.
