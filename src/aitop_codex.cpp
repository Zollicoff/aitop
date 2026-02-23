// src/aitop_codex.cpp
// Codex CLI log parser — reads JSONL files from ~/.codex/sessions/

#include "aitop_codex.hpp"

#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <vector>

namespace fs = std::filesystem;

namespace {

    // ── Model pricing (per million tokens) ──────────────────────────────
    // Codex models use OpenAI pricing.  Defaults: in=$2/M, out=$8/M.
    // Cached-input tokens are discounted; reasoning-output tokens cost more.

    struct Pricing { double in; double out; double cached_in; double reasoning_out; };

    Pricing pricing_for(const string& model) {
        // o3 / o4-mini reasoning models
        if (model.find("o3") != string::npos)
            return { 2.0, 8.0, 0.50, 12.0 };
        if (model.find("o4-mini") != string::npos)
            return { 1.10, 4.40, 0.275, 7.0 };
        // GPT-5 Codex (default)
        if (model.find("codex") != string::npos || model.find("gpt-5") != string::npos)
            return { 2.0, 8.0, 0.50, 8.0 };
        // GPT-4o family
        if (model.find("gpt-4o") != string::npos)
            return { 2.50, 10.0, 1.25, 10.0 };
        // Fallback
        return { 2.0, 8.0, 0.50, 8.0 };
    }

    double compute_cost(const string& model,
                        uint64_t in, uint64_t out,
                        uint64_t cached_in, uint64_t reasoning_out) {
        auto p = pricing_for(model);
        // input_tokens from Codex includes cached_input_tokens, so subtract
        // to avoid double-counting the cached portion.
        uint64_t non_cached_in = (in > cached_in) ? (in - cached_in) : 0;
        return (non_cached_in    * p.in  +
                cached_in        * p.cached_in +
                (out > reasoning_out ? (out - reasoning_out) : 0) * p.out +
                reasoning_out    * p.reasoning_out) / 1'000'000.0;
    }

    // ── Tiny helpers for JSON-less field extraction ──────────────────────

    // Return the value of "key": <number> (integer)
    uint64_t json_uint(const string& line, const string& key) {
        string needle = "\"" + key + "\":";
        auto pos = line.find(needle);
        if (pos == string::npos) return 0;
        pos += needle.size();
        while (pos < line.size() && (line[pos] == ' ' || line[pos] == '\t')) ++pos;
        uint64_t val = 0;
        while (pos < line.size() && line[pos] >= '0' && line[pos] <= '9') {
            val = val * 10 + static_cast<uint64_t>(line[pos] - '0');
            ++pos;
        }
        return val;
    }

    // Return the value of "key": <number> (floating point)
    double json_double(const string& line, const string& key) {
        string needle = "\"" + key + "\":";
        auto pos = line.find(needle);
        if (pos == string::npos) return -1.0;
        pos += needle.size();
        while (pos < line.size() && (line[pos] == ' ' || line[pos] == '\t')) ++pos;
        try {
            size_t consumed = 0;
            double val = std::stod(line.substr(pos, 30), &consumed);
            if (consumed > 0) return val;
        } catch (...) {}
        return -1.0;
    }

    // Return the value of "key": "string_value"
    string json_string(const string& line, const string& key) {
        string needle = "\"" + key + "\":\"";
        auto pos = line.find(needle);
        if (pos == string::npos) return {};
        pos += needle.size();
        auto end = line.find('"', pos);
        if (end == string::npos) return {};
        return line.substr(pos, end - pos);
    }

    // ── Recursive JSONL file finder ─────────────────────────────────────

    void find_jsonl(const fs::path& dir, vector<fs::path>& out) {
        std::error_code ec;
        for (auto& entry : fs::recursive_directory_iterator(dir, fs::directory_options::skip_permission_denied, ec)) {
            if (entry.is_regular_file(ec) && entry.path().extension() == ".jsonl")
                out.push_back(entry.path());
        }
    }

    // ── Project name from cwd (last path component) ────────────────────

    string project_name_from_cwd(const string& cwd) {
        if (cwd.empty()) return "unknown";
        string s = cwd;
        while (!s.empty() && s.back() == '/') s.pop_back();
        auto slash = s.rfind('/');
        if (slash == string::npos) return s.empty() ? "unknown" : s;
        string name = s.substr(slash + 1);
        return name.empty() ? "unknown" : name;
    }

    // ── Timestamp of last modification for is_active() ──────────────────

    std::chrono::system_clock::time_point newest_mtime;
    bool mtime_set = false;

    void track_mtime(const fs::path& p) {
        std::error_code ec;
        auto ftime = fs::last_write_time(p, ec);
        if (ec) return;
        auto sctp = std::chrono::clock_cast<std::chrono::system_clock>(ftime);
        if (!mtime_set || sctp > newest_mtime) {
            newest_mtime = sctp;
            mtime_set = true;
        }
    }

} // anonymous namespace

// ── Public API ──────────────────────────────────────────────────────────

namespace Codex {

AiTool::ToolStats collect() {
    AiTool::ToolStats stats;
    stats.type = AiTool::ToolType::Codex;

    // Reset mtime tracking
    mtime_set = false;

    const char* home = std::getenv("HOME");
    if (!home) return stats;

    fs::path sessions_dir = fs::path(home) / ".codex" / "sessions";
    if (!fs::is_directory(sessions_dir)) return stats;

    // Collect all JSONL files
    vector<fs::path> files;
    find_jsonl(sessions_dir, files);
    if (files.empty()) return stats;

    // Per-project aggregation
    struct ProjectAcc {
        string name;
        uint64_t input_tokens{};
        uint64_t output_tokens{};
        uint64_t cached_input_tokens{};
        uint64_t reasoning_output_tokens{};
        double cost{};
        std::unordered_set<string> sessions;
    };
    std::unordered_map<string, ProjectAcc> project_map;

    string latest_model;

    for (auto& filepath : files) {
        track_mtime(filepath);

        std::ifstream ifs(filepath);
        if (!ifs) continue;

        // Per-file state: session_meta gives us cwd/session_id once,
        // token_count events are cumulative so we keep the latest.
        string session_id;
        string session_cwd;
        string session_model;

        uint64_t last_input{};
        uint64_t last_output{};
        uint64_t last_cached_input{};
        uint64_t last_reasoning_output{};
        bool found_tokens = false;

        string line;
        while (std::getline(ifs, line)) {
            // Extract session metadata (appears once per file, near the top)
            if (line.find("\"session_meta\"") != string::npos) {
                string id = json_string(line, "id");
                if (!id.empty()) session_id = id;
                string cwd = json_string(line, "cwd");
                if (!cwd.empty()) session_cwd = cwd;
                continue;
            }

            // Extract model from turn_context events
            if (line.find("\"turn_context\"") != string::npos) {
                string model = json_string(line, "model");
                if (!model.empty()) session_model = model;
                continue;
            }

            // Extract cumulative token counts from token_count events
            if (line.find("\"token_count\"") != string::npos &&
                line.find("\"total_token_usage\"") != string::npos) {
                uint64_t in  = json_uint(line, "input_tokens");
                uint64_t out = json_uint(line, "output_tokens");
                uint64_t ci  = json_uint(line, "cached_input_tokens");
                uint64_t ro  = json_uint(line, "reasoning_output_tokens");

                if (in > 0 || out > 0) {
                    last_input            = in;
                    last_output           = out;
                    last_cached_input     = ci;
                    last_reasoning_output = ro;
                    found_tokens = true;
                }
            }
        }

        // After reading the whole file, aggregate cumulative totals
        if (!found_tokens) continue;

        if (session_id.empty()) {
            // Derive session_id from filename stem as fallback
            session_id = filepath.stem().string();
        }

        string proj = project_name_from_cwd(session_cwd);
        string model = session_model.empty() ? "gpt-5.3-codex" : session_model;

        double cost = compute_cost(model,
                                   last_input, last_output,
                                   last_cached_input, last_reasoning_output);

        auto& pa = project_map[proj];
        pa.name                   = proj;
        pa.input_tokens          += last_input;
        pa.output_tokens         += last_output;
        pa.cached_input_tokens   += last_cached_input;
        pa.reasoning_output_tokens += last_reasoning_output;
        pa.cost                  += cost;
        pa.sessions.insert(session_id);

        stats.total_tokens += last_input + last_output;
        stats.total_cost   += cost;

        if (!model.empty()) latest_model = model;
    }

    stats.active_model = latest_model;

    // Collect unique sessions across all projects
    std::unordered_set<string> all_sessions;
    for (auto& [_, pa] : project_map) {
        for (auto& s : pa.sessions) all_sessions.insert(s);
    }
    stats.total_sessions = static_cast<int>(all_sessions.size());

    // Build repos vector
    stats.repos.reserve(project_map.size());
    for (auto& [_, pa] : project_map) {
        AiTool::RepoUsage ru;
        ru.name              = pa.name;
        ru.input_tokens      = pa.input_tokens;
        ru.output_tokens     = pa.output_tokens;
        ru.cache_write_tokens = 0;  // Codex does not report cache writes
        ru.cache_read_tokens  = pa.cached_input_tokens;
        ru.total_tokens      = pa.input_tokens + pa.output_tokens;
        ru.estimated_cost    = pa.cost;
        ru.session_count     = static_cast<int>(pa.sessions.size());
        stats.repos.push_back(std::move(ru));
    }

    // Sort repos by cost descending
    std::sort(stats.repos.begin(), stats.repos.end(),
              [](const AiTool::RepoUsage& a, const AiTool::RepoUsage& b) {
                  return a.estimated_cost > b.estimated_cost;
              });

    stats.active_now = is_active();

    return stats;
}

bool is_active() {
    if (!mtime_set) {
        const char* home = std::getenv("HOME");
        if (!home) return false;
        fs::path sessions_dir = fs::path(home) / ".codex" / "sessions";
        if (!fs::is_directory(sessions_dir)) return false;

        vector<fs::path> files;
        find_jsonl(sessions_dir, files);
        for (auto& f : files) track_mtime(f);
    }
    if (!mtime_set) return false;

    auto now = std::chrono::system_clock::now();
    auto age = std::chrono::duration_cast<std::chrono::seconds>(now - newest_mtime).count();
    return age <= 60;
}

} // namespace Codex
