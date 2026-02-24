/* aitop - AI Tool Usage Monitor
   Three-band TUI layout with scrollable repo lists

   Licensed under the Apache License, Version 2.0
*/

#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

#include <fmt/format.h>

#include "aitop_draw.hpp"
#include "aitop_shared.hpp"
#include "btop_config.hpp"
#include "btop_draw.hpp"
#include "btop_theme.hpp"
#include "btop_tools.hpp"

using std::clamp;
using std::max;
using std::min;
using std::string;
using std::to_string;
using std::vector;

namespace AiDraw {

	//? Per-band scroll/selection state
	BandState claude_state;
	BandState codex_state;
	BandState gemini_state;
	int active_band = 0; // 0=Claude, 1=Codex, 2=Gemini

	//? Layout geometry (computed by calcSizes)
	namespace {
		struct BandGeom {
			int x{1};
			int y{1};
			int width{};
			int height{};
			int div_col{};    // x-position of the vertical divider column
			int repo_cols{};  // number of columns available for repo list text
			int stat_cols{};  // number of columns available for stats text
			int data_rows{};  // number of rows for repo data (excluding header)
		};

		BandGeom bands[3];
		bool sizes_valid = false;
	}

	//? ------------------------------------------------- FORMAT HELPERS ------------------------------------------------

	//* Format a token count: 1234567 -> "1.2M", 12345 -> "12.3K", 123 -> "123"
	static string format_tokens(uint64_t tokens) {
		if (tokens >= 1'000'000) {
			return fmt::format("{:.1f}M", static_cast<double>(tokens) / 1'000'000.0);
		} else if (tokens >= 1'000) {
			return fmt::format("{:.1f}K", static_cast<double>(tokens) / 1'000.0);
		}
		return to_string(tokens);
	}

	//* Format a cost value: 4.82 -> "$4.82"
	static string format_cost(double cost) {
		return fmt::format("${:.2f}", cost);
	}

	//? -------------------------------------------------- CALC SIZES --------------------------------------------------

	void calcSizes() {
		const int term_w = Term::width;
		const int term_h = Term::height;

		// Divide terminal into 3 equal horizontal bands
		const int band_h = term_h / 3;
		const int remainder = term_h - band_h * 3;

		for (int i = 0; i < 3; i++) {
			auto& b = bands[i];
			b.x = 1;
			b.width = term_w;
			// Give any extra rows to the last band
			b.height = band_h + (i == 2 ? remainder : 0);
			b.y = 1 + i * band_h;

			// Vertical divider splits the box roughly 35%/65%
			b.stat_cols = max(20, b.width * 35 / 100);
			b.div_col = b.x + b.stat_cols;
			b.repo_cols = b.width - b.stat_cols - 1; // -1 for the divider column itself

			// Data rows = inner height minus 1 row for the repo header
			// Inner height = height - 2 (top and bottom borders)
			b.data_rows = max(0, b.height - 2 - 1);
		}

		sizes_valid = true;
	}

	//? ------------------------------------------ DRAW A SINGLE BAND --------------------------------------------------

	//* Draw one band: border, stats on the left, scrollable repo list on the right
	static string drawBand(int band_idx, const AiTool::ToolStats& stats, BandState& state, bool is_active) {
		if (not sizes_valid) calcSizes();

		const auto& g = bands[band_idx];
		string out;
		out.reserve(g.width * g.height);

		// Pick band title
		string title;
		switch (stats.type) {
			case AiTool::ToolType::Claude: title = "Claude Code"; break;
			case AiTool::ToolType::Codex:  title = "Codex"; break;
			case AiTool::ToolType::Gemini: title = "Gemini"; break;
		}

		// Border color: highlight active band
		const string line_color = is_active ? Theme::c("hi_fg") : Theme::c("div_line");

		// Draw the box border with title
		out += Draw::createBox(g.x, g.y, g.width, g.height, line_color, true, title);

		//? ------- VERTICAL DIVIDER -------
		const string div_color = Theme::c("div_line");
		// Top junction
		out += Mv::to(g.y, g.div_col) + div_color + Symbols::div_up;
		// Vertical lines
		for (int row = g.y + 1; row < g.y + g.height - 1; row++) {
			out += Mv::to(row, g.div_col) + div_color + Symbols::v_line;
		}
		// Bottom junction
		out += Mv::to(g.y + g.height - 1, g.div_col) + div_color + Symbols::div_down;

		//? ------- LEFT HALF: Stats -------
		const string label_color = Theme::c("title");
		const string value_color = Theme::c("main_fg");
		const string hi_color = Theme::c("hi_fg");

		const int stat_x = g.x + 2; // 2 chars in from the left border
		const int stat_y = g.y + 1; // 1 row below top border
		const int label_w = 14;     // width for the label text

		// Total tokens
		out += Mv::to(stat_y + 0, stat_x) + Fx::b + label_color + "Total tokens:" + Fx::ub
			+ Mv::to(stat_y + 0, stat_x + label_w) + value_color + format_tokens(stats.total_tokens);

		// Estimated cost
		out += Mv::to(stat_y + 1, stat_x) + Fx::b + label_color + "Est. cost:" + Fx::ub
			+ Mv::to(stat_y + 1, stat_x + label_w) + value_color + format_cost(stats.total_cost);

		// Sessions
		out += Mv::to(stat_y + 2, stat_x) + Fx::b + label_color + "Sessions:" + Fx::ub
			+ Mv::to(stat_y + 2, stat_x + label_w) + value_color + to_string(stats.total_sessions);

		// Active now
		const bool active = stats.active_now;
		out += Mv::to(stat_y + 3, stat_x) + Fx::b + label_color + "Active now:" + Fx::ub
			+ Mv::to(stat_y + 3, stat_x + label_w) + (active ? hi_color : value_color)
			+ (active ? "Yes" : "No");

		// Active model (if active and we have room)
		if (active and not stats.active_model.empty() and g.height >= 8) {
			out += Mv::to(stat_y + 4, stat_x) + Fx::b + label_color + "Model:" + Fx::ub
				+ Mv::to(stat_y + 4, stat_x + label_w) + value_color
				+ Tools::uresize(stats.active_model, static_cast<size_t>(g.stat_cols - label_w - 4));
		}

		//? ------- RIGHT HALF: Scrollable repo list -------
		const int repo_x = g.div_col + 2;  // 2 chars right of divider
		const int repo_y = g.y + 1;        // 1 row below top border
		const int repo_w = g.repo_cols - 2; // usable width inside divider and right border

		const int nrepos = static_cast<int>(stats.repos.size());
		const int visible_rows = g.data_rows;

		// Clamp scroll and selection state
		if (nrepos == 0) {
			state.scroll_offset = 0;
			state.selected_row = -1;
		} else {
			state.scroll_offset = clamp(state.scroll_offset, 0, max(0, nrepos - visible_rows));
			if (state.selected_row >= 0) {
				state.selected_row = clamp(state.selected_row, 0, nrepos - 1);
			}
		}

		// Column widths for repo list: REPO  TOKENS  COST
		// Distribute available width: name gets most, tokens and cost get fixed widths
		const int cost_w = 8;
		const int token_w = 9;
		const int name_w = max(4, repo_w - cost_w - token_w - 2); // -2 for spacing

		// Header row
		const string header_color = Fx::b + Theme::c("title");
		out += Mv::to(repo_y, repo_x) + header_color
			+ Tools::ljust("REPO", name_w)
			+ Tools::rjust("TOKENS", token_w)
			+ Tools::rjust("COST", cost_w)
			+ Fx::ub;

		// Data rows
		const string sel_bg = Theme::c("selected_bg");
		const string sel_fg = Theme::c("selected_fg");
		const string reset = Fx::reset;

		for (int i = 0; i < visible_rows and i + state.scroll_offset < nrepos; i++) {
			const int repo_idx = i + state.scroll_offset;
			const auto& repo = stats.repos[repo_idx];
			const int row = repo_y + 1 + i;

			bool is_selected = is_active and state.selected_row == repo_idx;

			// Build row content
			string row_name = Tools::uresize(repo.name, static_cast<size_t>(name_w));
			string row_tokens = format_tokens(repo.total_tokens);
			string row_cost = format_cost(repo.estimated_cost);

			if (is_selected) {
				out += Mv::to(row, repo_x) + sel_bg + sel_fg
					+ Tools::ljust(row_name, name_w)
					+ Tools::rjust(row_tokens, token_w)
					+ Tools::rjust(row_cost, cost_w)
					+ reset;
			} else {
				out += Mv::to(row, repo_x) + value_color
					+ Tools::ljust(row_name, name_w)
					+ Tools::rjust(row_tokens, token_w)
					+ Tools::rjust(row_cost, cost_w);
			}
		}

		// Scroll indicator on the right border if there are more items than visible
		if (nrepos > visible_rows and visible_rows > 0) {
			const int scroll_range = g.height - 4; // available rows for indicator
			if (scroll_range > 0) {
				const int max_scroll = max(1, nrepos - visible_rows);
				const int indicator_pos = g.y + 2 + state.scroll_offset * scroll_range / max_scroll;
				out += Mv::to(clamp(indicator_pos, g.y + 2, g.y + g.height - 2),
							  g.x + g.width - 1)
					+ hi_color + Symbols::v_line;
			}
		}

		return out;
	}

	//? ------------------------------------------------- DRAW ALL BANDS -----------------------------------------------

	string drawAll(const AiTool::AitopData& data, bool force_redraw) {
		if (force_redraw or not sizes_valid) {
			calcSizes();
		}

		string out;
		out.reserve(Term::width * Term::height);

		// Band 0: Claude
		out += drawBand(0, data.claude, claude_state, active_band == 0);

		// Band 1: Codex
		out += drawBand(1, data.codex, codex_state, active_band == 1);

		// Band 2: Gemini
		out += drawBand(2, data.gemini, gemini_state, active_band == 2);

		out += Fx::reset;
		return out;
	}

	//? ----------------------------------------------- SELECTION / NAV ------------------------------------------------

	static BandState& getActiveBandState() {
		switch (active_band) {
			case 0: return claude_state;
			case 1: return codex_state;
			case 2: return gemini_state;
			default: return claude_state;
		}
	}

	//* Get the repo count for the active band (needs data, but we store enough state to navigate)
	//* We use selected_row clamp in drawBand to prevent overflow, so navigation
	//* can overshoot slightly -- drawBand will fix it on next render.

	void selection(const string& cmd) {
		if (not sizes_valid) calcSizes();

		auto& state = getActiveBandState();
		const int visible = bands[active_band].data_rows;

		if (cmd == "tab") {
			// Cycle active band: 0 -> 1 -> 2 -> 0
			active_band = (active_band + 1) % 3;
			// Ensure the new band has a valid selection
			auto& new_state = getActiveBandState();
			if (new_state.selected_row < 0) {
				new_state.selected_row = 0;
			}
			return;
		}

		// If no selection yet, start at first row
		if (state.selected_row < 0) {
			state.selected_row = 0;
		}

		if (cmd == "up") {
			if (state.selected_row > 0) {
				state.selected_row--;
				// Scroll up if selection goes above visible area
				if (state.selected_row < state.scroll_offset) {
					state.scroll_offset = state.selected_row;
				}
			}
		}
		else if (cmd == "down") {
			state.selected_row++;
			// Scroll down if selection goes below visible area
			if (state.selected_row >= state.scroll_offset + visible) {
				state.scroll_offset = state.selected_row - visible + 1;
			}
			// Note: clamp to actual repo count happens in drawBand
		}
		else if (cmd == "page_up") {
			state.selected_row = max(0, state.selected_row - visible);
			state.scroll_offset = max(0, state.scroll_offset - visible);
		}
		else if (cmd == "page_down") {
			state.selected_row += visible;
			state.scroll_offset += visible;
			// Clamp happens in drawBand when actual data is available
		}
	}

} // namespace AiDraw
