#pragma once

#include <string>
#include <vector>
#include <cstdint>
#include <deque>

using std::string;
using std::vector;
using std::deque;

namespace AiTool {

    enum class ToolType { Claude, Codex, Gemini };

    struct RepoUsage {
        string name;
        uint64_t input_tokens{};
        uint64_t output_tokens{};
        uint64_t cache_write_tokens{};
        uint64_t cache_read_tokens{};
        uint64_t total_tokens{};
        double estimated_cost{};
        int session_count{};
    };

    struct ModelPricing {
        string model_name;
        double input_rate{};
        double output_rate{};
        double cache_write_rate{};
        double cache_read_rate{};
    };

    struct LogEntry {
        string session_id;
        string model;
        string project_path;
        uint64_t input_tokens{};
        uint64_t output_tokens{};
        uint64_t cache_write_tokens{};
        uint64_t cache_read_tokens{};
        double cost{};
        int64_t timestamp{};
    };

    struct ToolStats {
        ToolType type;
        uint64_t total_tokens{};
        double total_cost{};
        int total_sessions{};
        bool active_now{};
        string active_model;
        vector<RepoUsage> repos;
    };

    struct AitopData {
        ToolStats claude;
        ToolStats codex;
        ToolStats gemini;
    };
}
