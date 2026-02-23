// src/aitop_claude.cpp
// Claude Code log parser — reads JSONL files from ~/.claude/projects/

#include "aitop_claude.hpp"

#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <functional>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <vector>

namespace fs = std::filesystem;

namespace {

    // ── Model pricing (per million tokens) ──────────────────────────────

    struct Pricing { double in; double out; double cw; double cr; };

    Pricing pricing_for(const string& model) {
        // Opus 4.5
        if (model.find("opus-4-5") != string::npos || model.find("opus-4.5") != string::npos)
            return { 5.0, 25.0, 6.25, 0.50 };
        // Opus 4 / 4.1 / 3  (match "opus" that is NOT 4.5)
        if (model.find("opus") != string::npos)
            return { 15.0, 75.0, 18.75, 1.50 };
        // Haiku 4.5
        if (model.find("haiku-4-5") != string::npos || model.find("haiku-4.5") != string::npos)
            return { 1.0, 5.0, 1.25, 0.10 };
        // Haiku 3.5
        if (model.find("haiku-3-5") != string::npos || model.find("haiku-3.5") != string::npos)
            return { 0.80, 4.0, 1.0, 0.08 };
        // Haiku 3
        if (model.find("haiku") != string::npos)
            return { 0.25, 1.25, 0.30, 0.03 };
        // Sonnet (default)
        return { 3.0, 15.0, 3.75, 0.30 };
    }

    double compute_cost(const string& model,
                        uint64_t in, uint64_t out,
                        uint64_t cw, uint64_t cr) {
        auto p = pricing_for(model);
        return (in  * p.in  +
                out * p.out +
                cw  * p.cw  +
                cr  * p.cr) / 1'000'000.0;
    }

    // ── Tiny helpers for JSON-less field extraction ──────────────────────

    // Return the value of "key": <number> (integer)
    uint64_t json_uint(const string& line, const string& key) {
        // Search for "key":
        string needle = "\"" + key + "\":";
        auto pos = line.find(needle);
        if (pos == string::npos) return 0;
        pos += needle.size();
        // Skip whitespace
        while (pos < line.size() && (line[pos] == ' ' || line[pos] == '\t')) ++pos;
        // Parse digits
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
        // Use stod on the substring
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
        // Strip trailing slash
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

namespace Claude {

AiTool::ToolStats collect() {
    AiTool::ToolStats stats;
    stats.type = AiTool::ToolType::Claude;

    // Reset mtime tracking
    mtime_set = false;

    // Locate projects directory
    const char* home = std::getenv("HOME");
    if (!home) return stats;

    fs::path projects_dir = fs::path(home) / ".claude" / "projects";
    if (!fs::is_directory(projects_dir)) return stats;

    // Collect all JSONL files
    vector<fs::path> files;
    find_jsonl(projects_dir, files);
    if (files.empty()) return stats;

    // Deduplication set: hash of message.id + requestId
    std::unordered_set<size_t> seen;
    std::hash<string> hasher;

    // Per-project aggregation
    struct ProjectAcc {
        string name;
        uint64_t input_tokens{};
        uint64_t output_tokens{};
        uint64_t cache_write_tokens{};
        uint64_t cache_read_tokens{};
        double cost{};
        std::unordered_set<string> sessions;
    };
    std::unordered_map<string, ProjectAcc> project_map;

    string latest_model;

    for (auto& filepath : files) {
        track_mtime(filepath);

        // Derive session_id from parent directory name (or filename stem)
        string session_id;
        string parent_name = filepath.parent_path().filename().string();
        if (parent_name == "subagents") {
            // For subagent files: .../session-id/subagents/agent-xxx.jsonl
            session_id = filepath.parent_path().parent_path().filename().string();
        } else {
            // For direct files: .../session-id.jsonl  (session id is the stem)
            // Or .../project-dir/session-id.jsonl
            session_id = filepath.stem().string();
        }

        std::ifstream ifs(filepath);
        if (!ifs) continue;

        string line;
        while (std::getline(ifs, line)) {
            // Fast reject: must contain input_tokens
            if (line.find("input_tokens") == string::npos) continue;

            // Must be an assistant message with usage data
            // Check for "message" with "usage" sub-object
            if (line.find("\"usage\"") == string::npos) continue;

            // Extract token counts from message.usage
            uint64_t in  = json_uint(line, "input_tokens");
            uint64_t out = json_uint(line, "output_tokens");
            uint64_t cw  = json_uint(line, "cache_creation_input_tokens");
            uint64_t cr  = json_uint(line, "cache_read_input_tokens");

            // Skip entries with no meaningful usage
            if (in == 0 && out == 0 && cw == 0 && cr == 0) continue;

            // Extract model, message id, requestId for dedup
            string model      = json_string(line, "model");
            string message_id = json_string(line, "id");
            string request_id = json_string(line, "requestId");

            // Deduplication: hash of message.id + requestId
            if (!message_id.empty() && !request_id.empty()) {
                size_t h = hasher(message_id + ":" + request_id);
                if (!seen.insert(h).second) continue; // duplicate
            }

            // Extract cwd for project name
            string cwd   = json_string(line, "cwd");
            string proj  = project_name_from_cwd(cwd);

            // Calculate cost
            double cost_usd = json_double(line, "costUSD");
            double cost = (cost_usd >= 0.0)
                        ? cost_usd
                        : compute_cost(model, in, out, cw, cr);

            // Aggregate into project
            auto& pa = project_map[proj];
            pa.name              = proj;
            pa.input_tokens     += in;
            pa.output_tokens    += out;
            pa.cache_write_tokens += cw;
            pa.cache_read_tokens  += cr;
            pa.cost             += cost;
            pa.sessions.insert(session_id);

            // Track totals
            stats.total_tokens += in + out + cw + cr;
            stats.total_cost   += cost;

            // Keep the model from the most-recently-parsed line as active model
            if (!model.empty()) latest_model = model;
        }
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
        ru.cache_write_tokens = pa.cache_write_tokens;
        ru.cache_read_tokens  = pa.cache_read_tokens;
        ru.total_tokens      = pa.input_tokens + pa.output_tokens +
                               pa.cache_write_tokens + pa.cache_read_tokens;
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
        // If collect() hasn't been called yet, do a quick scan
        const char* home = std::getenv("HOME");
        if (!home) return false;
        fs::path projects_dir = fs::path(home) / ".claude" / "projects";
        if (!fs::is_directory(projects_dir)) return false;

        vector<fs::path> files;
        find_jsonl(projects_dir, files);
        for (auto& f : files) track_mtime(f);
    }
    if (!mtime_set) return false;

    auto now = std::chrono::system_clock::now();
    auto age = std::chrono::duration_cast<std::chrono::seconds>(now - newest_mtime).count();
    return age <= 60;
}

} // namespace Claude
