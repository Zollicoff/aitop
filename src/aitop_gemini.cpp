// src/aitop_gemini.cpp
// Gemini CLI log parser — reads session JSON files from ~/.gemini/tmp/

#include "aitop_gemini.hpp"

#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <vector>

namespace fs = std::filesystem;

namespace {

    // ── Model pricing (per million tokens) ──────────────────────────────
    // Gemini 2.5 Pro / Flash pricing from Google.
    // "thoughts" (thinking) tokens are billed at the same rate as output.

    struct Pricing { double in; double out; };

    Pricing pricing_for(const string& model) {
        // Gemini 2.5 Pro
        if (model.find("2.5-pro") != string::npos || model.find("2-5-pro") != string::npos)
            return { 1.25, 10.0 };
        // Gemini 2.5 Flash
        if (model.find("2.5-flash") != string::npos || model.find("2-5-flash") != string::npos)
            return { 0.15, 0.60 };
        // Gemini 2.0 Flash
        if (model.find("2.0-flash") != string::npos || model.find("2-0-flash") != string::npos)
            return { 0.10, 0.40 };
        // Gemini 3 Flash (preview, seen in real logs: "gemini-3-flash-preview")
        if (model.find("3-flash") != string::npos)
            return { 0.15, 0.60 };
        // Gemini 3 Pro
        if (model.find("3-pro") != string::npos)
            return { 1.25, 10.0 };
        // Default: assume Flash-class pricing
        return { 0.15, 0.60 };
    }

    double compute_cost(const string& model,
                        uint64_t in, uint64_t out) {
        auto p = pricing_for(model);
        return (in * p.in + out * p.out) / 1'000'000.0;
    }

    // ── Tiny helpers for JSON-less field extraction ──────────────────────
    // Same pattern as aitop_claude.cpp / aitop_codex.cpp

    // Return the value of "key": <number> (integer)
    uint64_t json_uint(const string& text, const string& key) {
        string needle = "\"" + key + "\":";
        auto pos = text.find(needle);
        if (pos == string::npos) return 0;
        pos += needle.size();
        while (pos < text.size() && (text[pos] == ' ' || text[pos] == '\t')) ++pos;
        uint64_t val = 0;
        while (pos < text.size() && text[pos] >= '0' && text[pos] <= '9') {
            val = val * 10 + static_cast<uint64_t>(text[pos] - '0');
            ++pos;
        }
        return val;
    }

    // Return the value of "key": "string_value"
    string json_string(const string& text, const string& key) {
        string needle = "\"" + key + "\":\"";
        auto pos = text.find(needle);
        if (pos == string::npos) return {};
        pos += needle.size();
        auto end = text.find('"', pos);
        if (end == string::npos) return {};
        return text.substr(pos, end - pos);
    }

    // ── Project name from path (last path component) ────────────────────

    string project_name_from_path(const string& path) {
        if (path.empty()) return "unknown";
        string s = path;
        while (!s.empty() && s.back() == '/') s.pop_back();
        while (!s.empty() && s.back() == '\n') s.pop_back();
        auto slash = s.rfind('/');
        if (slash == string::npos) return s.empty() ? "unknown" : s;
        string name = s.substr(slash + 1);
        return name.empty() ? "unknown" : name;
    }

    // ── Read entire file into a string ──────────────────────────────────

    string read_file(const fs::path& p) {
        std::ifstream ifs(p);
        if (!ifs) return {};
        std::ostringstream oss;
        oss << ifs.rdbuf();
        return oss.str();
    }

    // ── Read .project_root to get project name ──────────────────────────

    string read_project_name(const fs::path& project_dir) {
        fs::path root_file = project_dir / ".project_root";
        std::error_code ec;
        if (!fs::is_regular_file(root_file, ec)) return project_dir.filename().string();
        string content = read_file(root_file);
        // Trim whitespace / newlines
        while (!content.empty() && (content.back() == '\n' || content.back() == '\r' || content.back() == ' '))
            content.pop_back();
        if (content.empty()) return project_dir.filename().string();
        return project_name_from_path(content);
    }

    // ── Find session JSON files in chats/ subdirectory ──────────────────

    void find_session_files(const fs::path& project_dir, vector<fs::path>& out) {
        fs::path chats_dir = project_dir / "chats";
        std::error_code ec;
        if (!fs::is_directory(chats_dir, ec)) return;
        for (auto& entry : fs::directory_iterator(chats_dir, fs::directory_options::skip_permission_denied, ec)) {
            if (entry.is_regular_file(ec) && entry.path().extension() == ".json") {
                auto fname = entry.path().filename().string();
                if (fname.find("session-") == 0) {
                    out.push_back(entry.path());
                }
            }
        }
    }

    // ── Extract token data from a session file ──────────────────────────
    //
    // Gemini session JSON format (from real data):
    //   { "sessionId": "...", "messages": [ ... ] }
    //
    // Each message of type "gemini" has a "tokens" object:
    //   "tokens": { "input": N, "output": N, "cached": N, "thoughts": N, "tool": N, "total": N }
    //
    // User messages have type "user" with content as an array of {text: ...}.
    // Info messages have type "info" with content as a plain string.
    //
    // Strategy: scan for "gemini" type messages and sum their token counts.
    // For user messages without token counts, estimate from content length.

    struct SessionTokens {
        string session_id;
        string model;
        uint64_t input_tokens{};
        uint64_t output_tokens{};  // output + thoughts (both are billed as output)
    };

    // Count characters in a "content" value — handles both string and array forms.
    // This is a rough helper: it just counts printable chars in the content region.
    uint64_t estimate_tokens_from_chars(uint64_t char_count) {
        // ~4 chars per token heuristic
        return char_count / 4;
    }

    SessionTokens parse_session(const fs::path& filepath) {
        SessionTokens result;
        string content = read_file(filepath);
        if (content.empty()) return result;

        // Extract session ID
        result.session_id = json_string(content, "sessionId");

        // We need to find all "gemini" type messages and their token blocks.
        // Since the file can be multi-line JSON, we scan for patterns.
        //
        // Strategy: find each "type":"gemini" occurrence and look for the
        // associated "tokens" block nearby.

        size_t pos = 0;
        while (pos < content.size()) {
            // Find next "type" field
            string type_needle = "\"type\":\"";
            auto type_pos = content.find(type_needle, pos);
            if (type_pos == string::npos) break;

            size_t type_val_start = type_pos + type_needle.size();
            auto type_val_end = content.find('"', type_val_start);
            if (type_val_end == string::npos) break;

            string msg_type = content.substr(type_val_start, type_val_end - type_val_start);
            pos = type_val_end + 1;

            if (msg_type == "gemini") {
                // Look for "tokens" block after this point (within a reasonable window)
                // Find the next "type" occurrence to limit our search scope
                auto next_type = content.find("\"type\":\"", pos);
                string search_region;
                if (next_type != string::npos) {
                    search_region = content.substr(pos, next_type - pos);
                } else {
                    search_region = content.substr(pos);
                }

                // Extract token counts from this gemini message's scope
                uint64_t in_tok  = json_uint(search_region, "input");
                uint64_t out_tok = json_uint(search_region, "output");
                uint64_t thoughts_tok = json_uint(search_region, "thoughts");

                // Extract model if present
                string model = json_string(search_region, "model");
                if (!model.empty()) result.model = model;

                if (in_tok > 0 || out_tok > 0 || thoughts_tok > 0) {
                    // Use actual token counts — thoughts are billed as output
                    result.input_tokens  += in_tok;
                    result.output_tokens += out_tok + thoughts_tok;
                } else {
                    // Fallback: estimate from content length
                    auto content_pos = search_region.find("\"content\":");
                    if (content_pos != string::npos) {
                        // Find the content value end (next major key or end)
                        auto end_pos = search_region.find("\"thoughts\":", content_pos);
                        if (end_pos == string::npos) end_pos = search_region.size();
                        uint64_t char_count = end_pos - content_pos;
                        result.output_tokens += estimate_tokens_from_chars(char_count);
                    }
                }

            } else if (msg_type == "user") {
                // User messages: estimate input tokens from content text length
                // Find the content for this user message
                auto next_type = content.find("\"type\":\"", pos);
                string search_region;
                if (next_type != string::npos) {
                    search_region = content.substr(pos, next_type - pos);
                } else {
                    search_region = content.substr(pos);
                }

                // Look for "text" field (user content is array of {text: ...})
                auto text_pos = search_region.find("\"text\":");
                if (text_pos != string::npos) {
                    // Find the quoted text value
                    auto quote_start = search_region.find('"', text_pos + 7);
                    if (quote_start != string::npos) {
                        auto quote_end = search_region.find('"', quote_start + 1);
                        if (quote_end != string::npos) {
                            uint64_t char_count = quote_end - quote_start - 1;
                            result.input_tokens += estimate_tokens_from_chars(char_count);
                        }
                    }
                }
            }
            // "info" type messages are system messages — skip them
        }

        return result;
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

    // ── Scan all session/log files for mtime tracking ───────────────────

    void scan_for_mtime(const fs::path& tmp_dir) {
        std::error_code ec;
        for (auto& entry : fs::recursive_directory_iterator(tmp_dir, fs::directory_options::skip_permission_denied, ec)) {
            if (entry.is_regular_file(ec) && entry.path().extension() == ".json")
                track_mtime(entry.path());
        }
    }

} // anonymous namespace

// ── Public API ──────────────────────────────────────────────────────────

namespace Gemini {

AiTool::ToolStats collect() {
    AiTool::ToolStats stats;
    stats.type = AiTool::ToolType::Gemini;

    // Reset mtime tracking
    mtime_set = false;

    const char* home = std::getenv("HOME");
    if (!home) return stats;

    fs::path tmp_dir = fs::path(home) / ".gemini" / "tmp";
    std::error_code ec;
    if (!fs::is_directory(tmp_dir, ec)) return stats;

    // Per-project aggregation
    struct ProjectAcc {
        string name;
        uint64_t input_tokens{};
        uint64_t output_tokens{};
        double cost{};
        std::unordered_set<string> sessions;
    };
    std::unordered_map<string, ProjectAcc> project_map;

    string latest_model;

    // Walk directories under ~/.gemini/tmp/
    for (auto& dir_entry : fs::directory_iterator(tmp_dir, fs::directory_options::skip_permission_denied, ec)) {
        if (!dir_entry.is_directory(ec)) continue;

        fs::path project_dir = dir_entry.path();

        // Skip the "bin" directory (contains bundled tools like rg)
        if (project_dir.filename() == "bin") continue;

        // Read project name from .project_root
        string proj = read_project_name(project_dir);

        // Find and parse session JSON files
        vector<fs::path> session_files;
        find_session_files(project_dir, session_files);

        // Also track mtime of logs.json
        fs::path logs_file = project_dir / "logs.json";
        if (fs::is_regular_file(logs_file, ec))
            track_mtime(logs_file);

        for (auto& session_path : session_files) {
            track_mtime(session_path);

            SessionTokens st = parse_session(session_path);
            if (st.session_id.empty()) continue;

            // Determine model and compute cost
            string model = st.model.empty() ? "gemini-2.5-flash" : st.model;
            double cost = compute_cost(model, st.input_tokens, st.output_tokens);

            // Aggregate into project
            auto& pa = project_map[proj];
            pa.name           = proj;
            pa.input_tokens  += st.input_tokens;
            pa.output_tokens += st.output_tokens;
            pa.cost          += cost;
            pa.sessions.insert(st.session_id);

            // Track totals
            stats.total_tokens += st.input_tokens + st.output_tokens;
            stats.total_cost   += cost;

            if (!st.model.empty()) latest_model = st.model;
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
        ru.cache_write_tokens = 0;
        ru.cache_read_tokens  = 0;
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
        fs::path tmp_dir = fs::path(home) / ".gemini" / "tmp";
        if (!fs::is_directory(tmp_dir)) return false;
        scan_for_mtime(tmp_dir);
    }
    if (!mtime_set) return false;

    auto now = std::chrono::system_clock::now();
    auto age = std::chrono::duration_cast<std::chrono::seconds>(now - newest_mtime).count();
    return age <= 60;
}

} // namespace Gemini
