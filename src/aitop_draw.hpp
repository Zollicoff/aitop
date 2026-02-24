/* aitop - AI Tool Usage Monitor
   Three-band TUI layout with scrollable repo lists

   Licensed under the Apache License, Version 2.0
*/

#pragma once

#include "aitop_shared.hpp"

#include <string>

namespace AiDraw {
	void calcSizes();

	std::string drawAll(const AiTool::AitopData& data, bool force_redraw = false);

	struct BandState {
		int scroll_offset{};
		int selected_row{};  // -1 = no selection
	};

	extern BandState claude_state;
	extern BandState codex_state;
	extern BandState gemini_state;
	extern int active_band; // 0=Claude, 1=Codex, 2=Gemini

	//? Selection/scroll control (called from input handler)
	void selection(const std::string& cmd);
}
