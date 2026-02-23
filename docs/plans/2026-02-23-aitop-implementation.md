# aitop Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Transform btop++ into aitop, a terminal-based AI coding tool usage monitor for Claude Code, Codex, and Gemini CLI.

**Architecture:** Replace btop's system data collectors with three AI tool log parsers. Replace the 5-box layout (CPU/Mem/Net/Proc/GPU) with a 3-band layout (Claude/Codex/Gemini). Reuse btop's rendering engine, scrollable list widget, theme system, and input handling. Each band has usage stats (left) and a scrollable repo list (right).

**Tech Stack:** C++23, CMake, btop's rendering engine, JSONL/JSON parsing via nlohmann/json or manual parsing.

---

## Phase 1: Strip btop down to a shell

### Task 1: Create aitop development branch

**Files:**
- None (git only)

**Step 1: Create and switch to dev branch**

Run: `cd ~/Github/aitop && git checkout -b dev`
Expected: Switched to new branch 'dev'

**Step 2: Commit**

No changes yet — branch creation only.

---

### Task 2: Define aitop data structures

**Files:**
- Create: `src/aitop_shared.hpp`
- Modify: `src/btop_shared.hpp` (reference only — do not edit yet)

**Step 1: Write the data structures header**

```cpp
// src/aitop_shared.hpp
#pragma once

#include <string>
#include <vector>
#include <cstdint>
#include <deque>

using std::string;
using std::vector;
using std::deque;

namespace AiTool {

    // Which tool this data belongs to
    enum class ToolType { Claude, Codex, Gemini };

    // Per-repo usage entry
    struct RepoUsage {
        string name;                // Repository/project name
        uint64_t input_tokens{};
        uint64_t output_tokens{};
        uint64_t cache_write_tokens{};
        uint64_t cache_read_tokens{};
        uint64_t total_tokens{};
        double estimated_cost{};    // USD
        int session_count{};
    };

    // Per-model pricing (per million tokens)
    struct ModelPricing {
        string model_name;
        double input_rate{};
        double output_rate{};
        double cache_write_rate{};
        double cache_read_rate{};
    };

    // Raw log entry parsed from local files
    struct LogEntry {
        string session_id;
        string model;
        string project_path;
        uint64_t input_tokens{};
        uint64_t output_tokens{};
        uint64_t cache_write_tokens{};
        uint64_t cache_read_tokens{};
        double cost{};              // Pre-calculated or estimated
        int64_t timestamp{};        // Unix ms
    };

    // Aggregated stats for one tool
    struct ToolStats {
        ToolType type;
        uint64_t total_tokens{};
        double total_cost{};
        int total_sessions{};
        bool active_now{};
        string active_model;        // Model currently in use (if active)
        vector<RepoUsage> repos;    // Sorted by cost descending
    };

    // All three tools
    struct AitopData {
        ToolStats claude;
        ToolStats codex;
        ToolStats gemini;
    };
}
```

**Step 2: Verify it compiles**

Run: `cd ~/Github/aitop && g++ -std=c++23 -fsyntax-only src/aitop_shared.hpp`
Expected: No output (clean compile)

**Step 3: Commit**

```bash
git add src/aitop_shared.hpp
git commit -m "feat: add aitop data structures for AI tool usage tracking"
```

---

### Task 3: Create Claude Code log parser

**Files:**
- Create: `src/aitop_claude.hpp`
- Create: `src/aitop_claude.cpp`

Claude Code logs live at `~/.claude/projects/`. The structure is:
- `~/.claude/projects/<encoded-project-path>/<session-id>/*.jsonl`
- Each JSONL line may contain `message.usage` with token counts
- Fields: `message.model`, `message.usage.input_tokens`, `message.usage.output_tokens`, `message.usage.cache_creation_input_tokens`, `message.usage.cache_read_input_tokens`, `sessionId`, `costUSD`, `cwd`

Reference: `~/Github/Claude_Code_CLI_Usage/src/services/logParser.ts`

**Step 1: Write the header**

```cpp
// src/aitop_claude.hpp
#pragma once

#include "aitop_shared.hpp"

namespace Claude {
    // Parse all Claude Code logs and return aggregated stats
    AiTool::ToolStats collect();

    // Check if a Claude Code session is currently active
    bool is_active();
}
```

**Step 2: Write the implementation**

```cpp
// src/aitop_claude.cpp
#include "aitop_claude.hpp"

#include <filesystem>
#include <fstream>
#include <sstream>
#include <unordered_map>
#include <unordered_set>
#include <algorithm>
#include <cstdlib>

namespace fs = std::filesystem;

namespace {

    // Simple JSON value extraction (avoids adding a JSON library dependency)
    // Extracts a string value for a given key from a JSON line
    string extract_string(const string& line, const string& key) {
        string search = "\"" + key + "\":\"";
        auto pos = line.find(search);
        if (pos == string::npos) {
            // Try without quotes (for numeric fields serialized as strings)
            search = "\"" + key + "\":";
            pos = line.find(search);
            if (pos == string::npos) return "";
            pos += search.size();
            auto end = line.find_first_of(",}", pos);
            if (end == string::npos) return "";
            string val = line.substr(pos, end - pos);
            // Remove surrounding quotes if present
            if (val.size() >= 2 && val.front() == '"' && val.back() == '"')
                val = val.substr(1, val.size() - 2);
            return val;
        }
        pos += search.size();
        auto end = line.find('"', pos);
        if (end == string::npos) return "";
        return line.substr(pos, end - pos);
    }

    // Extract a numeric value for a given key
    double extract_number(const string& line, const string& key) {
        string search = "\"" + key + "\":";
        auto pos = line.find(search);
        if (pos == string::npos) return 0.0;
        pos += search.size();
        // Skip whitespace
        while (pos < line.size() && (line[pos] == ' ' || line[pos] == '\t')) pos++;
        auto end = line.find_first_of(",} \t\n", pos);
        if (end == string::npos) return 0.0;
        try {
            return std::stod(line.substr(pos, end - pos));
        } catch (...) {
            return 0.0;
        }
    }

    // Claude model pricing (per million tokens)
    double get_cost(const string& model, uint64_t input, uint64_t output,
                    uint64_t cache_write, uint64_t cache_read) {
        double in_rate = 3.0, out_rate = 15.0, cw_rate = 3.75, cr_rate = 0.30; // Sonnet default

        string m = model;
        std::transform(m.begin(), m.end(), m.begin(), ::tolower);

        if (m.find("opus") != string::npos) {
            if (m.find("4.5") != string::npos || m.find("4-5") != string::npos) {
                in_rate = 5.0; out_rate = 25.0; cw_rate = 6.25; cr_rate = 0.50;
            } else {
                in_rate = 15.0; out_rate = 75.0; cw_rate = 18.75; cr_rate = 1.50;
            }
        } else if (m.find("haiku") != string::npos) {
            if (m.find("4.5") != string::npos || m.find("4-5") != string::npos) {
                in_rate = 1.0; out_rate = 5.0; cw_rate = 1.25; cr_rate = 0.10;
            } else if (m.find("3.5") != string::npos || m.find("3-5") != string::npos) {
                in_rate = 0.80; out_rate = 4.0; cw_rate = 1.0; cr_rate = 0.08;
            } else {
                in_rate = 0.25; out_rate = 1.25; cw_rate = 0.30; cr_rate = 0.03;
            }
        }
        // Sonnet rates are the default

        return (input * in_rate + output * out_rate +
                cache_write * cw_rate + cache_read * cr_rate) / 1'000'000.0;
    }

    // Extract project name from path (last component)
    string project_name(const string& path) {
        if (path.empty()) return "unknown";
        auto pos = path.find_last_of('/');
        if (pos == string::npos) return path;
        string name = path.substr(pos + 1);
        return name.empty() ? "unknown" : name;
    }

} // anonymous namespace

namespace Claude {

    AiTool::ToolStats collect() {
        AiTool::ToolStats stats;
        stats.type = AiTool::ToolType::Claude;

        string home = std::getenv("HOME") ? std::getenv("HOME") : "";
        fs::path projects_dir = fs::path(home) / ".claude" / "projects";

        if (!fs::exists(projects_dir)) return stats;

        std::unordered_map<string, AiTool::RepoUsage> repo_map;
        std::unordered_set<string> seen_sessions;
        std::unordered_set<string> seen_entries; // dedup key

        // Walk all JSONL files
        for (auto& entry : fs::recursive_directory_iterator(projects_dir)) {
            if (!entry.is_regular_file()) continue;
            if (entry.path().extension() != ".jsonl") continue;

            // Session ID from parent directory name
            string session_id = entry.path().parent_path().filename().string();

            std::ifstream file(entry.path());
            string line;
            while (std::getline(file, line)) {
                // Skip lines without usage data
                if (line.find("\"input_tokens\"") == string::npos) continue;

                uint64_t input = static_cast<uint64_t>(extract_number(line, "input_tokens"));
                uint64_t output = static_cast<uint64_t>(extract_number(line, "output_tokens"));
                uint64_t cache_write = static_cast<uint64_t>(extract_number(line, "cache_creation_input_tokens"));
                uint64_t cache_read = static_cast<uint64_t>(extract_number(line, "cache_read_input_tokens"));

                // Skip zero-token entries
                if (input == 0 && output == 0) continue;

                // Dedup using message.id + requestId
                string msg_id = extract_string(line, "id");
                string req_id = extract_string(line, "requestId");
                string dedup_key = msg_id + ":" + req_id;
                if (!dedup_key.empty() && dedup_key != ":") {
                    if (seen_entries.count(dedup_key)) continue;
                    seen_entries.insert(dedup_key);
                }

                string model = extract_string(line, "model");
                string cwd = extract_string(line, "cwd");
                string proj = project_name(cwd);

                // Cost: use costUSD if available, otherwise calculate
                double cost = extract_number(line, "costUSD");
                if (cost <= 0.0) {
                    cost = get_cost(model, input, output, cache_write, cache_read);
                }

                // Aggregate into repo
                auto& repo = repo_map[proj];
                repo.name = proj;
                repo.input_tokens += input;
                repo.output_tokens += output;
                repo.cache_write_tokens += cache_write;
                repo.cache_read_tokens += cache_read;
                repo.total_tokens += input + output;
                repo.estimated_cost += cost;

                // Track sessions
                if (!session_id.empty()) {
                    seen_sessions.insert(session_id);
                    // Track per-repo sessions (approximate)
                    // Not perfectly accurate but good enough
                }

                stats.total_tokens += input + output;
                stats.total_cost += cost;
            }
        }

        stats.total_sessions = static_cast<int>(seen_sessions.size());

        // Convert repo_map to sorted vector
        for (auto& [name, repo] : repo_map) {
            repo.session_count = 0; // TODO: per-repo session tracking
            stats.repos.push_back(std::move(repo));
        }
        std::sort(stats.repos.begin(), stats.repos.end(),
                  [](const auto& a, const auto& b) { return a.estimated_cost > b.estimated_cost; });

        stats.active_now = is_active();

        return stats;
    }

    bool is_active() {
        // Check if any claude process is running
        // Simple heuristic: check for claude lock files or running processes
        string home = std::getenv("HOME") ? std::getenv("HOME") : "";
        fs::path claude_dir = fs::path(home) / ".claude";

        // Check for recent JSONL file modifications (within last 60 seconds)
        fs::path projects_dir = claude_dir / "projects";
        if (!fs::exists(projects_dir)) return false;

        auto now = fs::file_time_type::clock::now();
        for (auto& entry : fs::recursive_directory_iterator(projects_dir)) {
            if (!entry.is_regular_file()) continue;
            if (entry.path().extension() != ".jsonl") continue;
            auto mod_time = entry.last_write_time();
            auto age = std::chrono::duration_cast<std::chrono::seconds>(now - mod_time);
            if (age.count() < 60) return true;
        }
        return false;
    }

} // namespace Claude
```

**Step 3: Verify it compiles (syntax only)**

Run: `cd ~/Github/aitop && g++ -std=c++23 -fsyntax-only -I src src/aitop_claude.cpp`
Expected: Clean compile (no output)

**Step 4: Commit**

```bash
git add src/aitop_claude.hpp src/aitop_claude.cpp
git commit -m "feat: add Claude Code log parser with pricing and deduplication"
```

---

### Task 4: Create Codex log parser

**Files:**
- Create: `src/aitop_codex.hpp`
- Create: `src/aitop_codex.cpp`

Codex logs live at `~/.codex/`. Key files:
- `~/.codex/sessions/YYYY/MM/DD/rollout-TIMESTAMP-UUID.jsonl` — detailed session recordings
- `~/.codex/history.jsonl` — user message history with session_id and timestamp
- Session JSONL contains `session_meta` and `response_item` events

Note: Codex session files may not contain explicit token counts like Claude does. We may need to estimate from message lengths or extract from response metadata. The implementation should handle both cases.

**Step 1: Write the header**

```cpp
// src/aitop_codex.hpp
#pragma once

#include "aitop_shared.hpp"

namespace Codex {
    AiTool::ToolStats collect();
    bool is_active();
}
```

**Step 2: Write the implementation**

```cpp
// src/aitop_codex.cpp
#include "aitop_codex.hpp"

#include <filesystem>
#include <fstream>
#include <sstream>
#include <unordered_map>
#include <unordered_set>
#include <algorithm>
#include <cstdlib>

namespace fs = std::filesystem;

namespace {

    string extract_string(const string& line, const string& key) {
        string search = "\"" + key + "\":\"";
        auto pos = line.find(search);
        if (pos == string::npos) {
            search = "\"" + key + "\":";
            pos = line.find(search);
            if (pos == string::npos) return "";
            pos += search.size();
            auto end = line.find_first_of(",}", pos);
            if (end == string::npos) return "";
            string val = line.substr(pos, end - pos);
            if (val.size() >= 2 && val.front() == '"' && val.back() == '"')
                val = val.substr(1, val.size() - 2);
            return val;
        }
        pos += search.size();
        auto end = line.find('"', pos);
        if (end == string::npos) return "";
        return line.substr(pos, end - pos);
    }

    double extract_number(const string& line, const string& key) {
        string search = "\"" + key + "\":";
        auto pos = line.find(search);
        if (pos == string::npos) return 0.0;
        pos += search.size();
        while (pos < line.size() && (line[pos] == ' ' || line[pos] == '\t')) pos++;
        auto end = line.find_first_of(",} \t\n", pos);
        if (end == string::npos) return 0.0;
        try { return std::stod(line.substr(pos, end - pos)); }
        catch (...) { return 0.0; }
    }

    // OpenAI Codex pricing (per million tokens) - gpt-5.3-codex default
    double get_cost(uint64_t input, uint64_t output) {
        // Using approximate rates for codex models
        double in_rate = 2.0, out_rate = 8.0;
        return (input * in_rate + output * out_rate) / 1'000'000.0;
    }

    string project_name_from_path(const string& path) {
        if (path.empty()) return "unknown";
        auto pos = path.find_last_of('/');
        if (pos == string::npos) return path;
        string name = path.substr(pos + 1);
        return name.empty() ? "unknown" : name;
    }

} // anonymous namespace

namespace Codex {

    AiTool::ToolStats collect() {
        AiTool::ToolStats stats;
        stats.type = AiTool::ToolType::Codex;

        string home = std::getenv("HOME") ? std::getenv("HOME") : "";
        fs::path sessions_dir = fs::path(home) / ".codex" / "sessions";

        if (!fs::exists(sessions_dir)) return stats;

        std::unordered_map<string, AiTool::RepoUsage> repo_map;
        std::unordered_set<string> seen_sessions;

        // Walk all session JSONL files
        for (auto& entry : fs::recursive_directory_iterator(sessions_dir)) {
            if (!entry.is_regular_file()) continue;
            if (entry.path().extension() != ".jsonl") continue;

            std::ifstream file(entry.path());
            string line;
            string current_session;
            string current_project;

            while (std::getline(file, line)) {
                // Extract session metadata
                if (line.find("\"session_meta\"") != string::npos ||
                    line.find("\"cwd\"") != string::npos) {
                    string cwd = extract_string(line, "cwd");
                    if (!cwd.empty()) current_project = project_name_from_path(cwd);
                    string sid = extract_string(line, "session_id");
                    if (!sid.empty()) {
                        current_session = sid;
                        seen_sessions.insert(sid);
                    }
                }

                // Look for token usage in response items
                if (line.find("\"input_tokens\"") != string::npos ||
                    line.find("\"output_tokens\"") != string::npos) {
                    uint64_t input = static_cast<uint64_t>(extract_number(line, "input_tokens"));
                    uint64_t output = static_cast<uint64_t>(extract_number(line, "output_tokens"));

                    if (input == 0 && output == 0) continue;

                    double cost = get_cost(input, output);
                    string proj = current_project.empty() ? "unknown" : current_project;

                    auto& repo = repo_map[proj];
                    repo.name = proj;
                    repo.input_tokens += input;
                    repo.output_tokens += output;
                    repo.total_tokens += input + output;
                    repo.estimated_cost += cost;

                    stats.total_tokens += input + output;
                    stats.total_cost += cost;
                }
            }
        }

        stats.total_sessions = static_cast<int>(seen_sessions.size());

        for (auto& [name, repo] : repo_map) {
            stats.repos.push_back(std::move(repo));
        }
        std::sort(stats.repos.begin(), stats.repos.end(),
                  [](const auto& a, const auto& b) { return a.estimated_cost > b.estimated_cost; });

        stats.active_now = is_active();

        return stats;
    }

    bool is_active() {
        string home = std::getenv("HOME") ? std::getenv("HOME") : "";
        fs::path sessions_dir = fs::path(home) / ".codex" / "sessions";
        if (!fs::exists(sessions_dir)) return false;

        auto now = fs::file_time_type::clock::now();
        for (auto& entry : fs::recursive_directory_iterator(sessions_dir)) {
            if (!entry.is_regular_file()) continue;
            if (entry.path().extension() != ".jsonl") continue;
            auto age = std::chrono::duration_cast<std::chrono::seconds>(now - entry.last_write_time());
            if (age.count() < 60) return true;
        }
        return false;
    }

} // namespace Codex
```

**Step 3: Verify it compiles**

Run: `cd ~/Github/aitop && g++ -std=c++23 -fsyntax-only -I src src/aitop_codex.cpp`
Expected: Clean compile

**Step 4: Commit**

```bash
git add src/aitop_codex.hpp src/aitop_codex.cpp
git commit -m "feat: add Codex CLI log parser"
```

---

### Task 5: Create Gemini log parser

**Files:**
- Create: `src/aitop_gemini.hpp`
- Create: `src/aitop_gemini.cpp`

Gemini logs live at `~/.gemini/`. Key files:
- `~/.gemini/tmp/<project>/logs.json` — JSON array of message objects with sessionId, type, message, timestamp
- `~/.gemini/tmp/<project>/chats/session-*.json` — per-session JSON with messages array
- Gemini does NOT log token counts directly — we estimate from message character lengths

**Step 1: Write the header**

```cpp
// src/aitop_gemini.hpp
#pragma once

#include "aitop_shared.hpp"

namespace Gemini {
    AiTool::ToolStats collect();
    bool is_active();
}
```

**Step 2: Write the implementation**

```cpp
// src/aitop_gemini.cpp
#include "aitop_gemini.hpp"

#include <filesystem>
#include <fstream>
#include <sstream>
#include <unordered_map>
#include <unordered_set>
#include <algorithm>
#include <cstdlib>

namespace fs = std::filesystem;

namespace {

    string extract_string(const string& line, const string& key) {
        string search = "\"" + key + "\":\"";
        auto pos = line.find(search);
        if (pos == string::npos) return "";
        pos += search.size();
        auto end = line.find('"', pos);
        if (end == string::npos) return "";
        return line.substr(pos, end - pos);
    }

    // Rough token estimate: ~4 chars per token (standard approximation)
    uint64_t estimate_tokens(const string& text) {
        return text.size() / 4;
    }

    // Gemini pricing (per million tokens) - Gemini 2.5 Pro default
    double get_cost(uint64_t input, uint64_t output) {
        double in_rate = 1.25, out_rate = 10.0;
        return (input * in_rate + output * out_rate) / 1'000'000.0;
    }

    string project_name_from_root(const fs::path& project_root_file) {
        std::ifstream f(project_root_file);
        string path;
        std::getline(f, path);
        if (path.empty()) return "unknown";
        auto pos = path.find_last_of('/');
        if (pos == string::npos) return path;
        string name = path.substr(pos + 1);
        return name.empty() ? "unknown" : name;
    }

} // anonymous namespace

namespace Gemini {

    AiTool::ToolStats collect() {
        AiTool::ToolStats stats;
        stats.type = AiTool::ToolType::Gemini;

        string home = std::getenv("HOME") ? std::getenv("HOME") : "";
        fs::path gemini_tmp = fs::path(home) / ".gemini" / "tmp";

        if (!fs::exists(gemini_tmp)) return stats;

        std::unordered_map<string, AiTool::RepoUsage> repo_map;
        std::unordered_set<string> seen_sessions;

        // Walk project directories in tmp/
        for (auto& proj_entry : fs::directory_iterator(gemini_tmp)) {
            if (!proj_entry.is_directory()) continue;

            // Get project name from .project_root file
            string proj_name = "unknown";
            fs::path root_file = proj_entry.path() / ".project_root";
            if (fs::exists(root_file)) {
                proj_name = project_name_from_root(root_file);
            } else {
                proj_name = proj_entry.path().filename().string();
            }

            // Parse chat session files
            fs::path chats_dir = proj_entry.path() / "chats";
            if (fs::exists(chats_dir)) {
                for (auto& chat_entry : fs::directory_iterator(chats_dir)) {
                    if (!chat_entry.is_regular_file()) continue;
                    if (chat_entry.path().extension() != ".json") continue;

                    std::ifstream file(chat_entry.path());
                    string content((std::istreambuf_iterator<char>(file)),
                                    std::istreambuf_iterator<char>());

                    // Extract session ID
                    string session_id = extract_string(content, "sessionId");
                    if (!session_id.empty()) seen_sessions.insert(session_id);

                    // Estimate tokens from messages
                    // Count user messages as input, assistant messages as output
                    uint64_t input_tokens = 0, output_tokens = 0;

                    // Simple approach: scan for type/message pairs
                    size_t pos = 0;
                    while ((pos = content.find("\"type\"", pos)) != string::npos) {
                        string type = extract_string(content.substr(pos, 200), "type");
                        size_t msg_pos = content.find("\"content\"", pos);
                        if (msg_pos == string::npos) msg_pos = content.find("\"message\"", pos);
                        if (msg_pos != string::npos && msg_pos - pos < 500) {
                            // Find message text boundaries
                            auto start = content.find(":\"", msg_pos);
                            if (start != string::npos) {
                                start += 2;
                                auto end = content.find("\"", start);
                                if (end != string::npos) {
                                    string msg_text = content.substr(start, end - start);
                                    uint64_t tokens = estimate_tokens(msg_text);
                                    if (type == "user" || type == "info") {
                                        input_tokens += tokens;
                                    } else {
                                        output_tokens += tokens;
                                    }
                                }
                            }
                        }
                        pos += 6; // advance past "type"
                    }

                    double cost = get_cost(input_tokens, output_tokens);

                    auto& repo = repo_map[proj_name];
                    repo.name = proj_name;
                    repo.input_tokens += input_tokens;
                    repo.output_tokens += output_tokens;
                    repo.total_tokens += input_tokens + output_tokens;
                    repo.estimated_cost += cost;

                    stats.total_tokens += input_tokens + output_tokens;
                    stats.total_cost += cost;
                }
            }

            // Also parse logs.json if present
            fs::path logs_file = proj_entry.path() / "logs.json";
            if (fs::exists(logs_file)) {
                std::ifstream file(logs_file);
                string content((std::istreambuf_iterator<char>(file)),
                                std::istreambuf_iterator<char>());

                // Extract sessions from logs
                size_t pos = 0;
                while ((pos = content.find("\"sessionId\"", pos)) != string::npos) {
                    string sid = extract_string(content.substr(pos, 200), "sessionId");
                    if (!sid.empty()) seen_sessions.insert(sid);
                    pos += 11;
                }
            }
        }

        stats.total_sessions = static_cast<int>(seen_sessions.size());

        for (auto& [name, repo] : repo_map) {
            stats.repos.push_back(std::move(repo));
        }
        std::sort(stats.repos.begin(), stats.repos.end(),
                  [](const auto& a, const auto& b) { return a.estimated_cost > b.estimated_cost; });

        stats.active_now = is_active();

        return stats;
    }

    bool is_active() {
        string home = std::getenv("HOME") ? std::getenv("HOME") : "";
        fs::path gemini_tmp = fs::path(home) / ".gemini" / "tmp";
        if (!fs::exists(gemini_tmp)) return false;

        auto now = fs::file_time_type::clock::now();
        for (auto& entry : fs::recursive_directory_iterator(gemini_tmp)) {
            if (!entry.is_regular_file()) continue;
            if (entry.path().extension() != ".json") continue;
            auto age = std::chrono::duration_cast<std::chrono::seconds>(now - entry.last_write_time());
            if (age.count() < 60) return true;
        }
        return false;
    }

} // namespace Gemini
```

**Step 3: Verify it compiles**

Run: `cd ~/Github/aitop && g++ -std=c++23 -fsyntax-only -I src src/aitop_gemini.cpp`
Expected: Clean compile

**Step 4: Commit**

```bash
git add src/aitop_gemini.hpp src/aitop_gemini.cpp
git commit -m "feat: add Gemini CLI log parser with token estimation"
```

---

### Task 6: Gut btop's data collection — replace with aitop collectors

**Files:**
- Modify: `src/btop.cpp` — replace Runner thread logic
- Modify: `src/btop_shared.hpp` — add aitop includes, keep namespaces for draw compatibility
- Modify: `CMakeLists.txt` — swap source files

This is the critical integration step. We remove the system monitoring code and wire in our three parsers.

**Step 1: Update CMakeLists.txt**

Replace the platform-specific source files in `libbtop` OBJECT library with our new files. Remove the linux/osx/freebsd/openbsd/netbsd collectors. Add `aitop_claude.cpp`, `aitop_codex.cpp`, `aitop_gemini.cpp`.

In `CMakeLists.txt`, find the `add_library(libbtop OBJECT ...)` block (~lines 48-59) and replace:
- Remove: all `src/linux/btop_collect.cpp` and other platform collector entries
- Add: `src/aitop_claude.cpp`, `src/aitop_codex.cpp`, `src/aitop_gemini.cpp`
- Keep: all other src/*.cpp files (btop_draw, btop_input, btop_config, btop_theme, btop_tools, btop_menu, btop_shared, btop_cli, btop_log, btop.cpp)

Also remove the platform-specific `#if defined(...)` source blocks lower in the file.

**Step 2: Modify Runner thread in btop.cpp**

In `src/btop.cpp`, find `Runner::_runner()` (line ~462). Replace the collect/draw calls:

Instead of:
```cpp
if (v_contains(conf.boxes, "cpu"))  { ... Cpu::collect() ... Cpu::draw() ... }
if (v_contains(conf.boxes, "mem"))  { ... Mem::collect() ... Mem::draw() ... }
if (v_contains(conf.boxes, "net"))  { ... Net::collect() ... Net::draw() ... }
if (v_contains(conf.boxes, "proc")) { ... Proc::collect() ... Proc::draw() ... }
```

Replace with:
```cpp
#include "aitop_claude.hpp"
#include "aitop_codex.hpp"
#include "aitop_gemini.hpp"

// In Runner::_runner():
auto claude_stats = Claude::collect();
auto codex_stats = Codex::collect();
auto gemini_stats = Gemini::collect();

// Draw calls will come in Task 7
```

**Step 3: Verify build compiles (may have link errors — that's expected at this stage)**

Run: `cd ~/Github/aitop && mkdir -p build && cd build && cmake .. -DBTOP_GPU=OFF 2>&1 | tail -5`
Expected: CMake configuration completes

**Step 4: Commit**

```bash
git add -A
git commit -m "refactor: replace system collectors with aitop AI tool parsers"
```

---

### Task 7: Create the three-band layout and drawing code

**Files:**
- Create: `src/aitop_draw.hpp`
- Create: `src/aitop_draw.cpp`
- Modify: `src/btop_draw.cpp` — reference for reusing `Draw::createBox()`, `Proc::draw()` patterns

This is the largest task. We create a new draw module that renders three horizontal bands, each with stats on the left and a scrollable repo list on the right. We reuse btop's `Draw::createBox()`, `Mv::to()`, `Fx::` color codes, and `Theme::c()` / `Theme::g()` color system.

**Step 1: Write the header**

```cpp
// src/aitop_draw.hpp
#pragma once

#include "aitop_shared.hpp"
#include <string>

namespace AiDraw {
    // Calculate band sizes based on terminal dimensions
    void calcSizes();

    // Draw one tool band (stats left, repo list right)
    std::string drawBand(const AiTool::ToolStats& stats, const std::string& title,
                         int x, int y, int width, int height,
                         int scroll_offset, int selected_row);

    // Draw all three bands
    std::string drawAll(const AiTool::AitopData& data, bool force_redraw = false);

    // Band navigation state
    struct BandState {
        int scroll_offset{};
        int selected_row{};
    };

    extern BandState claude_state;
    extern BandState codex_state;
    extern BandState gemini_state;
    extern int active_band; // 0=Claude, 1=Codex, 2=Gemini
}
```

**Step 2: Write the implementation**

This is a large file. Key components:
- `calcSizes()`: Divide terminal height by 3 for equal bands
- `drawBand()`: Uses `Draw::createBox()` for border, then renders stats on left half and scrollable repo list on right half
- `drawAll()`: Calls `drawBand()` three times and concatenates output
- Scrollable list: Mirrors `Proc::selection()` logic with start/selected/select_max

The implementation should follow the patterns in `btop_draw.cpp` lines 1569-2600 (Proc::draw). Use `Mv::to()` for cursor positioning, `Theme::c()` for colors, and `Fx::b`/`Fx::ub` for bold.

```cpp
// src/aitop_draw.cpp
#include "aitop_draw.hpp"
#include "btop_draw.hpp"
#include "btop_tools.hpp"
#include "btop_theme.hpp"

#include <fmt/core.h>
#include <algorithm>

using namespace std;

namespace AiDraw {

    BandState claude_state;
    BandState codex_state;
    BandState gemini_state;
    int active_band = 0;

    namespace {
        int band_x, band_width;
        int band_heights[3];
        int band_y[3];
    }

    void calcSizes() {
        int total_h = Term::height;
        int total_w = Term::width;

        band_x = 1;
        band_width = total_w;

        // Divide height equally among 3 bands
        int each = total_h / 3;
        int remainder = total_h % 3;

        band_heights[0] = each + (remainder > 0 ? 1 : 0);
        band_heights[1] = each + (remainder > 1 ? 1 : 0);
        band_heights[2] = each;

        band_y[0] = 1;
        band_y[1] = band_y[0] + band_heights[0];
        band_y[2] = band_y[1] + band_heights[1];
    }

    string format_tokens(uint64_t tokens) {
        if (tokens >= 1'000'000) return fmt::format("{:.1f}M", tokens / 1'000'000.0);
        if (tokens >= 1'000) return fmt::format("{:.1f}K", tokens / 1'000.0);
        return fmt::format("{}", tokens);
    }

    string format_cost(double cost) {
        return fmt::format("${:.2f}", cost);
    }

    string drawBand(const AiTool::ToolStats& stats, const string& title,
                    int x, int y, int width, int height,
                    int scroll_offset, int selected_row) {
        string out;

        // Draw box border with title
        out += Draw::createBox(x, y, width, height, "", false, title, "", 0);

        int inner_w = width - 2;
        int inner_h = height - 2;
        int left_w = inner_w / 2;
        int right_w = inner_w - left_w;
        int content_x = x + 1;
        int content_y = y + 1;

        // === LEFT HALF: Usage Stats ===
        string active_str = stats.active_now ? (Fx::b + Theme::c("hi_fg") + "Yes" + Fx::ub + Fx::reset) : "No";

        if (inner_h >= 1)
            out += Mv::to(content_y, content_x) + Theme::c("title") + " Total tokens: " + Fx::reset + format_tokens(stats.total_tokens);
        if (inner_h >= 2)
            out += Mv::to(content_y + 1, content_x) + Theme::c("title") + " Est. cost:   " + Fx::reset + format_cost(stats.total_cost);
        if (inner_h >= 3)
            out += Mv::to(content_y + 2, content_x) + Theme::c("title") + " Sessions:    " + Fx::reset + to_string(stats.total_sessions);
        if (inner_h >= 4)
            out += Mv::to(content_y + 3, content_x) + Theme::c("title") + " Active now:  " + Fx::reset + active_str;

        // === Vertical divider ===
        int div_x = content_x + left_w;
        for (int row = 0; row < inner_h; row++) {
            out += Mv::to(content_y + row, div_x) + Theme::c("div_line") + Symbols::v_line;
        }

        // === RIGHT HALF: Repo List ===
        int list_x = div_x + 1;
        int list_w = right_w - 1;

        // Header
        if (inner_h >= 1) {
            string header = fmt::format(" {:<{}} {:>8} {:>8}",
                "REPO", max(1, list_w - 20), "TOKENS", "COST");
            if ((int)header.size() > list_w) header.resize(list_w);
            out += Mv::to(content_y, list_x) + Fx::b + Theme::c("title") + header + Fx::ub + Fx::reset;
        }

        // Scrollable repo rows
        int list_rows = inner_h - 1; // minus header
        int total_repos = static_cast<int>(stats.repos.size());
        int visible_start = scroll_offset;
        int visible_end = min(visible_start + list_rows, total_repos);

        for (int i = visible_start; i < visible_end; i++) {
            int row = content_y + 1 + (i - visible_start);
            const auto& repo = stats.repos[i];

            bool is_selected = (i == selected_row);
            string bg = is_selected ? Theme::c("selected_bg") : "";
            string fg = is_selected ? Theme::c("hi_fg") : Fx::reset;

            string line = fmt::format(" {:<{}} {:>8} {:>8}",
                repo.name.substr(0, max(1, list_w - 20)),
                max(1, list_w - 20),
                format_tokens(repo.total_tokens),
                format_cost(repo.estimated_cost));
            if ((int)line.size() > list_w) line.resize(list_w);

            out += Mv::to(row, list_x) + bg + fg + line + Fx::reset;
        }

        return out;
    }

    string drawAll(const AiTool::AitopData& data, bool force_redraw) {
        string out;

        if (force_redraw) {
            out += Term::clear;
            calcSizes();
        }

        out += drawBand(data.claude, "Claude Code", band_x, band_y[0], band_width, band_heights[0],
                        claude_state.scroll_offset, claude_state.selected_row);
        out += drawBand(data.codex, "Codex", band_x, band_y[1], band_width, band_heights[1],
                        codex_state.scroll_offset, codex_state.selected_row);
        out += drawBand(data.gemini, "Gemini", band_x, band_y[2], band_width, band_heights[2],
                        gemini_state.scroll_offset, gemini_state.selected_row);

        return out;
    }

} // namespace AiDraw
```

**Step 3: Add to CMakeLists.txt**

Add `src/aitop_draw.cpp` to the `libbtop` OBJECT library sources.

**Step 4: Verify build**

Run: `cd ~/Github/aitop/build && cmake .. -DBTOP_GPU=OFF && make -j$(nproc) 2>&1 | tail -20`
Expected: May have some link errors from removed btop code that still references old namespaces — fix iteratively.

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add three-band TUI layout with scrollable repo lists"
```

---

### Task 8: Wire drawing into Runner thread and input handling

**Files:**
- Modify: `src/btop.cpp` — Runner thread calls AiDraw::drawAll()
- Modify: `src/btop_input.cpp` — Replace Proc navigation with band navigation

**Step 1: Update Runner thread**

In `Runner::_runner()`, after collecting data from all three parsers, call:

```cpp
AiTool::AitopData data;
data.claude = Claude::collect();
data.codex = Codex::collect();
data.gemini = Gemini::collect();

output += AiDraw::drawAll(data, conf.force_redraw);
```

**Step 2: Update input handling**

In `btop_input.cpp`, replace the Proc-specific input handling with band navigation:

- Tab: cycle `AiDraw::active_band` between 0, 1, 2
- Up/Down: scroll the active band's repo list
- Page Up/Page Down: page scroll
- q: quit (keep existing)
- Escape/m: menu (keep existing)

**Step 3: Update calcSizes**

In `btop.cpp` where `Draw::calcSizes()` is called, also call `AiDraw::calcSizes()`.

**Step 4: Test the full build and run**

Run: `cd ~/Github/aitop/build && make -j$(nproc) && ./btop`
Expected: Three bands visible with tool names. Data populates from local logs.

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: wire aitop draw and input into main loop"
```

---

### Task 9: Remove dead btop code

**Files:**
- Delete: `src/linux/btop_collect.cpp` and all platform collector directories
- Modify: `src/btop_shared.hpp` — remove CPU/Mem/Net/Proc/GPU data structures that are no longer used
- Modify: `src/btop_draw.cpp` — remove Cpu::draw, Mem::draw, Net::draw, Proc::draw functions
- Modify: `src/btop_menu.cpp` — simplify or remove system-monitoring menu items

**Step 1: Delete platform collectors**

```bash
rm -rf src/linux/ src/osx/ src/freebsd/ src/openbsd/ src/netbsd/
```

**Step 2: Strip unused structs from btop_shared.hpp**

Remove: `cpu_info`, `mem_info`, `net_info`, `proc_info`, `gpu_info`, `disk_info` and their namespace declarations. Keep the namespace shells if needed for `Draw::` functions that reference them.

**Step 3: Strip unused draw functions from btop_draw.cpp**

Remove: `Cpu::draw()`, `Mem::draw()`, `Net::draw()`, `Proc::draw()`, `Gpu::draw()`. Keep `Draw::createBox()`, `Draw::Meter`, `Draw::Graph`, `Draw::TextEdit` and utility functions.

**Step 4: Build and verify**

Run: `cd ~/Github/aitop/build && cmake .. -DBTOP_GPU=OFF && make -j$(nproc)`
Expected: Clean build with no system monitoring code

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor: remove all btop system monitoring code"
```

---

### Task 10: Rename binary and branding

**Files:**
- Modify: `CMakeLists.txt` — rename output binary from `btop` to `aitop`
- Modify: `src/btop.cpp` — update version string and program name
- Modify: `src/btop_config.cpp` — update config file path from `~/.config/btop/` to `~/.config/aitop/`

**Step 1: Rename in CMakeLists.txt**

Change `project(btop ...)` to `project(aitop ...)` and executable name.

**Step 2: Update branding in btop.cpp**

Find the version/title string and change "btop++" to "aitop".

**Step 3: Update config path**

Change config directory from `btop` to `aitop`.

**Step 4: Build and verify**

Run: `cd ~/Github/aitop/build && cmake .. -DBTOP_GPU=OFF && make -j$(nproc) && ls -la aitop`
Expected: `aitop` binary exists

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor: rename to aitop with updated branding and config paths"
```

---

### Task 11: Test with real data and polish

**Files:**
- Various — fix issues found during testing

**Step 1: Run aitop and verify Claude Code data**

Run: `cd ~/Github/aitop/build && ./aitop`
Expected: Claude band shows real data from `~/.claude/projects/`

**Step 2: Verify Codex data**

Expected: Codex band shows data from `~/.codex/sessions/`

**Step 3: Verify Gemini data**

Expected: Gemini band shows data from `~/.gemini/tmp/`

**Step 4: Test keyboard navigation**

- Tab between bands
- Arrow keys to scroll repo lists
- q to quit

**Step 5: Fix any rendering issues**

Iterate on layout, column widths, color choices.

**Step 6: Commit**

```bash
git add -A
git commit -m "fix: polish layout and fix rendering issues from testing"
```

---

### Task 12: Push to GitHub

**Step 1: Push dev branch**

Run: `cd ~/Github/aitop && git push -u origin dev`

**Step 2: Update README**

Write a brief README.md explaining what aitop is, how to build it, and how to use it.

**Step 3: Commit and push**

```bash
git add README.md
git commit -m "docs: add aitop README"
git push
```
