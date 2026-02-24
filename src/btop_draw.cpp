/* Copyright 2021 Aristocratos (jakob@qvantnet.com)

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

	   http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.

indent = tab
tab-size = 4
*/

#include <algorithm>
#include <array>
#include <cmath>
#include <iterator>
#include <ranges>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>

#include <fmt/format.h>

#include "btop_config.hpp"
#include "btop_draw.hpp"
#include "btop_input.hpp"
#include "btop_log.hpp"
#include "btop_menu.hpp"
#include "btop_shared.hpp"
#include "btop_theme.hpp"
#include "btop_tools.hpp"

using std::array;
using std::clamp;
using std::cmp_equal;
using std::cmp_greater;
using std::cmp_less;
using std::cmp_less_equal;
using std::floor;
using std::max;
using std::min;
using std::round;
using std::to_string;
using std::views::iota;

using namespace Tools;
using namespace std::literals; // for operator""s
namespace rng = std::ranges;

namespace Symbols {
	const string meter = "■";

	const array<string, 10> superscript = { "⁰", "¹", "²", "³", "⁴", "⁵", "⁶", "⁷", "⁸", "⁹" };

	const std::unordered_map<string, vector<string>> graph_symbols = {
		{ "braille_up", {
			" ", "⢀", "⢠", "⢰", "⢸",
			"⡀", "⣀", "⣠", "⣰", "⣸",
			"⡄", "⣄", "⣤", "⣴", "⣼",
			"⡆", "⣆", "⣦", "⣶", "⣾",
			"⡇", "⣇", "⣧", "⣷", "⣿"
		}},
		{"braille_down", {
			" ", "⠈", "⠘", "⠸", "⢸",
			"⠁", "⠉", "⠙", "⠹", "⢹",
			"⠃", "⠋", "⠛", "⠻", "⢻",
			"⠇", "⠏", "⠟", "⠿", "⢿",
			"⡇", "⡏", "⡟", "⡿", "⣿"
		}},
		{"block_up", {
			" ", "▗", "▗", "▐", "▐",
			"▖", "▄", "▄", "▟", "▟",
			"▖", "▄", "▄", "▟", "▟",
			"▌", "▙", "▙", "█", "█",
			"▌", "▙", "▙", "█", "█"
		}},
		{"block_down", {
			" ", "▝", "▝", "▐", "▐",
			"▘", "▀", "▀", "▜", "▜",
			"▘", "▀", "▀", "▜", "▜",
			"▌", "▛", "▛", "█", "█",
			"▌", "▛", "▛", "█", "█"
		}},
		{"tty_up", {
			" ", "░", "░", "▒", "▒",
			"░", "░", "▒", "▒", "█",
			"░", "▒", "▒", "▒", "█",
			"▒", "▒", "▒", "█", "█",
			"▒", "█", "█", "█", "█"
		}},
		{"tty_down", {
			" ", "░", "░", "▒", "▒",
			"░", "░", "▒", "▒", "█",
			"░", "▒", "▒", "▒", "█",
			"▒", "▒", "▒", "█", "█",
			"▒", "█", "█", "█", "█"
		}},
	};
}

namespace Draw {

	string banner_gen(int y, int x, bool centered, bool redraw) {
		static string banner;
		static size_t width = 0;
		if (redraw) banner.clear();
		if (banner.empty()) {
			string b_color, bg, fg, oc, letter;
			auto lowcolor = Config::getB("lowcolor");
			auto tty_mode = Config::getB("tty_mode");
			for (size_t z = 0; const auto& line : Global::Banner_src) {
				if (const auto w = ulen(line[1]); w > width) width = w;
				if (tty_mode) {
					fg = (z > 2) ? "\x1b[31m" : "\x1b[91m";
					bg = (z > 2) ? "\x1b[90m" : "\x1b[37m";
				}
				else {
					fg = Theme::hex_to_color(line[0], lowcolor);
					int bg_i = 120 - z * 12;
					bg = Theme::dec_to_color(bg_i, bg_i, bg_i, lowcolor);
				}
				for (size_t i = 0; i < line[1].size(); i += 3) {
					if (line[1][i] == ' ') {
						letter = Mv::r(1);
						i -= 2;
					}
					else
						letter = line[1].substr(i, 3);

					b_color = (letter == "█") ? fg : bg;
					if (b_color != oc) banner += b_color;
					banner += letter;
					oc = b_color;
				}
				if (++z < Global::Banner_src.size()) banner += Mv::l(ulen(line[1])) + Mv::d(1);
			}
			banner += Mv::r(18 - Global::Version.size())
					+ Theme::c("main_fg") + Fx::b + Fx::i + "v" + Global::Version + Fx::reset;
		}
		if (redraw) return "";
		return (centered ? Mv::to(y, Term::width / 2 - width / 2) : Mv::to(y, x)) + banner;
	}

	TextEdit::TextEdit() {}
	TextEdit::TextEdit(string text, bool numeric) : numeric(numeric), text(std::move(text)) {
		pos = this->text.size();
		upos = ulen(this->text);
	}

	bool TextEdit::command(const std::string_view key) {
		if (key == "left" and upos > 0) {
			upos--;
			pos = uresize(text, upos).size();
		}
		else if (key == "right" and pos < text.size()) {
			upos++;
			pos = uresize(text, upos).size();
		}
		else if (key == "home" and not text.empty() and pos > 0) {
			pos = upos = 0;
		}
		else if (key == "end" and not text.empty() and pos < text.size()) {
			pos = text.size();
			upos = ulen(text);
		}
		else if (key == "backspace" and pos > 0) {
			if (pos == text.size()) {
				text = uresize(text, --upos);
				pos = text.size();
			}
			else {
				const string first = uresize(text, --upos);
				pos = first.size();
				text = first + luresize(text.substr(pos), ulen(text) - upos - 1);
			}
		}
		else if (key == "delete" and pos < text.size()) {
			const string first = uresize(text, upos + 1);
			text = uresize(first, ulen(first) - 1) + text.substr(first.size());
		}
		else if (key == "space" and not numeric) {
			text.insert(pos++, 1, ' ');
			upos++;
		}
		else if (ulen(key) == 1 and text.size() < text.max_size() - 20) {
			if (numeric and not isint(key)) return false;
			if (key.size() == 1) {
				text.insert(pos++, 1, key.at(0));
				upos++;
			}
			else {
				const auto first = fmt::format("{}{}", uresize(text, upos), key);
				text = first + text.substr(pos);
				upos++;
				pos = first.size();
			}
		}
		else
			return false;

		return true;
	}

	string TextEdit::operator()(const size_t limit) {
		string out;
		size_t c_upos = upos;
		if (text.empty())
			return Fx::ul + " " + Fx::uul;
		if (limit > 0 and ulen(text) + 1 > limit) {
			try {
				const size_t half = (size_t)round((double)limit / 2);
				string first;

				if (upos + half > ulen(text))
					first = luresize(text.substr(0, pos), limit - (ulen(text) - upos));
				else if (upos - half < 1)
					first = text.substr(0, pos);
				else
					first = luresize(text.substr(0, pos), half);

				out = first + uresize(text.substr(pos), limit - ulen(first));
				c_upos = ulen(first);
			}
			catch (const std::exception& e) {
				Logger::error("In TextEdit::operator() : {}", e.what());
				return "";
			}
		}
		else
			out = text;

		if (c_upos == 0)
			return Fx::ul + uresize(out, 1) + Fx::uul + luresize(out, ulen(out) - 1);
		else if (c_upos == ulen(out))
			return out + Fx::ul + " " + Fx::uul;
		else
			return uresize(out, c_upos) + Fx::ul + luresize(uresize(out, c_upos + 1), 1) + Fx::uul + luresize(out, ulen(out) - c_upos - 1);
	}

	void TextEdit::clear() {
		this->text.clear();
	}

	string createBox(
			const int x, const int y, const int width, const int height, string line_color, bool fill, const std::string_view title,
			const std::string_view title2, const int num
	) {
		string out;

		if (line_color.empty())
			line_color = Theme::c("div_line");

		auto tty_mode = Config::getB("tty_mode");
		auto rounded = Config::getB("rounded_corners");
		const string numbering = (num == 0) ? "" : Theme::c("hi_fg") + (tty_mode ? std::to_string(num) : Symbols::superscript.at(clamp(num, 0, 9)));
		const auto& right_up = (tty_mode or not rounded ? Symbols::right_up : Symbols::round_right_up);
		const auto& left_up = (tty_mode or not rounded ? Symbols::left_up : Symbols::round_left_up);
		const auto& right_down = (tty_mode or not rounded ? Symbols::right_down : Symbols::round_right_down);
		const auto& left_down = (tty_mode or not rounded ? Symbols::left_down : Symbols::round_left_down);

		out = Fx::reset + line_color;

		//? Draw horizontal lines
		for (const int& hpos : {y, y + height - 1}) {
			out += Mv::to(hpos, x) + Symbols::h_line * (width - 1);
		}

		//? Draw vertical lines and fill if enabled
		for (const int& hpos : iota(y + 1, y + height - 1)) {
			out += Mv::to(hpos, x) + Symbols::v_line
				+  ((fill) ? string(width - 2, ' ') : Mv::r(width - 2))
				+  Symbols::v_line;
		}

		//? Draw corners
		out += 	Mv::to(y, x) + left_up
			+	Mv::to(y, x + width - 1) + right_up
			+	Mv::to(y + height - 1, x) +left_down
			+	Mv::to(y + height - 1, x + width - 1) + right_down;

		//? Draw titles if defined
		if (not title.empty()) {
			out += fmt::format(
				"{}{}{}{}{}{}{}{}{}", Mv::to(y, x + 2), Symbols::title_left, Fx::b, numbering, Theme::c("title"), title, Fx::ub,
				line_color, Symbols::title_right
			);
		}
		if (not title2.empty()) {
			out += fmt::format(
				"{}{}{}{}{}{}{}{}{}", Mv::to(y + height - 1, x + 2), Symbols::title_left_down, Fx::b, numbering, Theme::c("title"), title2, Fx::ub,
				line_color, Symbols::title_right_down
			);
		}

		return out + Fx::reset + Mv::to(y + 1, x + 1);
	}

	bool update_clock(bool force) {
		const auto& clock_format = Config::getS("clock_format");
		if (not Cpu::shown or clock_format.empty()) {
			if (clock_format.empty() and not Global::clock.empty()) Global::clock.clear();
			return false;
		}

		static const std::unordered_map<string, string> clock_custom_format = {
			{"/user", Tools::username()},
			{"/host", Tools::hostname()},
			{"/uptime", ""}
		};

		static time_t c_time{};
		static size_t clock_len{};
		static string clock_str;

		if (auto n_time = time(nullptr); not force and n_time == c_time)
			return false;
		else {
			c_time = n_time;
			const auto new_clock = Tools::strf_time(clock_format);
			if (not force and new_clock == clock_str) return false;
			clock_str = new_clock;
		}

		auto& out = Global::clock;
		auto cpu_bottom = Config::getB("cpu_bottom");
		const auto& x = Cpu::x;
		const auto y = (cpu_bottom ? Cpu::y + Cpu::height - 1 : Cpu::y);
		const auto& width = Cpu::width;
		const auto& title_left = (cpu_bottom ? Symbols::title_left_down : Symbols::title_left);
		const auto& title_right = (cpu_bottom ? Symbols::title_right_down : Symbols::title_right);


		for (const auto& [c_format, replacement] : clock_custom_format) {
			if (clock_str.contains(c_format)) {
				if (c_format == "/uptime") {
					string upstr = sec_to_dhms(system_uptime());
					if (upstr.size() > 8) upstr.resize(upstr.size() - 3);
					clock_str = s_replace(clock_str, c_format, upstr);
				}
				else {
					clock_str = s_replace(clock_str, c_format, replacement);
				}
			}

		}

		clock_str = uresize(clock_str, std::max(10, width - 66 - (Term::width >= 100 and Config::getB("show_battery") and Cpu::has_battery ? 22 : 0)));
		out.clear();

		if (clock_str.size() != clock_len) {
			if (not Global::resized and clock_len > 0)
				out = Mv::to(y, x+(width / 2)-(clock_len / 2)) + Fx::ub + Theme::c("cpu_box") + Symbols::h_line * clock_len;
			clock_len = clock_str.size();
		}

		out += Mv::to(y, x+(width / 2)-(clock_len / 2)) + Fx::ub + Theme::c("cpu_box") + title_left
			+ Theme::c("title") + Fx::b + clock_str + Theme::c("cpu_box") + Fx::ub + title_right;

		return true;
	}

	//* Meter class ------------------------------------------------------------------------------------------------------------>
	Meter::Meter() {}

	Meter::Meter(const int width, string color_gradient, bool invert)
		: width(width), color_gradient(std::move(color_gradient)), invert(invert) {}

	string Meter::operator()(int value) {
		if (width < 1) return "";
		value = clamp(value, 0, 100);
		if (not cache.at(value).empty()) return cache.at(value);
		auto& out = cache.at(value);
		for (const int& i : iota(1, width + 1)) {
			int y = round((double)i * 100.0 / width);
			if (value >= y)
				out += Theme::g(color_gradient).at(invert ? 100 - y : y) + Symbols::meter;
			else {
				out += Theme::c("meter_bg") + Symbols::meter * (width + 1 - i);
				break;
			}
		}
		out += Fx::reset;
		return out;
	}

	//* Graph class ------------------------------------------------------------------------------------------------------------>
	void Graph::_create(const deque<long long>& data, int data_offset) {
		bool mult = (data.size() - data_offset > 1);
		const auto& graph_symbol = Symbols::graph_symbols.at(symbol + '_' + (invert ? "down" : "up"));
		array<int, 2> result;
		const float mod = (height == 1) ? 0.3 : 0.1;
		long long data_value = 0;
		if (mult and data_offset > 0) {
			last = data.at(data_offset - 1);
			if (max_value > 0) last = clamp((last + offset) * 100 / max_value, 0ll, 100ll);
		}

		//? Horizontal iteration over values in <data>
		for (const int& i : iota(data_offset, (int)data.size())) {
			if (not tty_mode and mult) current = not current;
			if (i < 0) {
				data_value = 0;
				last = 0;
			}
			else {
				data_value = data.at(i);
				if (max_value > 0) data_value = clamp((data_value + offset) * 100 / max_value, 0ll, 100ll);
			}

			//? Vertical iteration over height of graph
			for (const int& horizon : iota(0, height)) {
				const int cur_high = (height > 1) ? round(100.0 * (height - horizon) / height) : 100;
				const int cur_low = (height > 1) ? round(100.0 * (height - (horizon + 1)) / height) : 0;
				//? Calculate previous + current value to fit two values in 1 braille character
				for (int ai = 0; const auto& value : {last, data_value}) {
					const int clamp_min = (no_zero and horizon == height - 1 and not (mult and i == data_offset and ai == 0)) ? 1 : 0;
					if (value >= cur_high)
						result[ai++] = 4;
					else if (value <= cur_low)
						result[ai++] = clamp_min;
					else {
						result[ai++] = clamp((int)round((float)(value - cur_low) * 4 / (cur_high - cur_low) + mod), clamp_min, 4);
					}
				}
				//? Generate graph symbol from 5x5 2D vector
				if (height == 1) {
					if (result.at(0) + result.at(1) == 0) graphs.at(current).at(horizon) += Mv::r(1);
					else {
						if (not color_gradient.empty()) graphs.at(current).at(horizon) += Theme::g(color_gradient).at(clamp(max(last, data_value), 0ll, 100ll));
						graphs.at(current).at(horizon) += graph_symbol.at((result.at(0) * 5 + result.at(1)));
					}
				}
				else graphs.at(current).at(horizon) += graph_symbol.at((result.at(0) * 5 + result.at(1)));
			}
			if (mult and i >= 0) last = data_value;
		}
		last = data_value;
		out.clear();
		if (height == 1) {
			out += graphs.at(current).at(0);
		}
		else {
			for (const int& i : iota(1, height + 1)) {
				if (i > 1) out += Mv::d(1) + Mv::l(width);
				if (not color_gradient.empty())
					out += (invert) ? Theme::g(color_gradient).at(i * 100 / height) : Theme::g(color_gradient).at(100 - ((i - 1) * 100 / height));
				out += (invert) ? graphs.at(current).at(height - i) : graphs.at(current).at(i-1);
			}
		}
		if (not color_gradient.empty()) out += Fx::reset;
	}

	Graph::Graph() {}

	Graph::Graph(int width, int height, const string& color_gradient,
				 const deque<long long>& data, const string& symbol,
				 bool invert, bool no_zero, long long max_value, long long offset)
	: width(width), height(height), color_gradient(color_gradient),
	  invert(invert), no_zero(no_zero), offset(offset) {
		if (Config::getB("tty_mode") or symbol == "tty") this->symbol = "tty";
		else if (symbol != "default") this->symbol = symbol;
		else this->symbol = Config::getS("graph_symbol");
		if (this->symbol == "tty") tty_mode = true;

		if (max_value == 0 and offset > 0) max_value = 100;
		this->max_value = max_value;
		const int value_width = (tty_mode ? data.size() : ceil((double)data.size() / 2));
		int data_offset = (value_width > width) ? data.size() - width * (tty_mode ? 1 : 2) : 0;

		if (not tty_mode and (data.size() - data_offset) % 2 != 0) {
			data_offset--;
		}

		//? Populate the two switching graph vectors and fill empty space if data size < width
		for (const int& i : iota(0, height * 2)) {
			if (tty_mode and i % 2 != current) continue;
			graphs[(i % 2 != 0)].push_back((value_width < width) ? ((height == 1) ? Mv::r(1) : " "s) * (width - value_width) : "");
		}
		if (data.size() == 0) return;
		this->_create(data, data_offset);
	}

	string& Graph::operator()(const deque<long long>& data, bool data_same) {
		if (data_same) return out;

		//? Make room for new characters on graph
		if (not tty_mode) current = not current;
		for (const int& i : iota(0, height)) {
			if (height == 1 and graphs.at(current).at(i).at(1) == '[') {
				if (graphs.at(current).at(i).at(3) == 'C') graphs.at(current).at(i).erase(0, 4);
				else graphs.at(current).at(i).erase(0, graphs.at(current).at(i).find_first_of('m') + 4);
			}
			else if (graphs.at(current).at(i).at(0) == ' ') graphs.at(current).at(i).erase(0, 1);
			else graphs.at(current).at(i).erase(0, 3);
		}
		this->_create(data, (int)data.size() - 1);
		return out;
	}

	string& Graph::operator()() {
		return out;
	}
	//*------------------------------------------------------------------------------------------------------------------------->

}

//? ============================================ Namespace variable definitions ============================================
//? These are kept because Draw::calcSizes() and btop_tools.cpp::get_min_size() reference them.
//? The draw functions are stubbed out since aitop uses AiDraw instead.

namespace Cpu {
	int width_p = 100, height_p = 32;
	int min_width = 60, min_height = 8;
	int x = 1, y = 1, width = 20, height;
	int b_columns, b_column_size;
	int b_x, b_y, b_width, b_height;
	bool shown = true, redraw = true;
	string box;

	string draw(const cpu_info&,
#if defined(GPU_SUPPORT)
		const vector<Gpu::gpu_info>&,
#endif
		bool, bool) { return ""; }
}

#ifdef GPU_SUPPORT
namespace Gpu {
	int width_p = 100, height_p = 32;
	int min_width = 41, min_height = 8;
	int width = 41, total_height;
	vector<int> x_vec = {}, y_vec = {};
	int b_width;
	vector<int> b_x_vec = {}, b_y_vec = {};
	vector<int> b_height_vec = {};
	vector<bool> redraw = {};
	int shown = 0;
	int count = 0;
	vector<int> shown_panels = {};
	vector<string> box = {};

	string draw(const gpu_info&, unsigned long, bool, bool) { return ""; }
}
#endif

namespace Mem {
	int width_p = 45, height_p = 40;
	int min_width = 36, min_height = 10;
	int x = 1, y, width = 20, height;
	int mem_width, disks_width, divider, item_height, mem_size, mem_meter, graph_height, disk_meter;
	bool shown = true, redraw = true;
	string box;

	string draw(const mem_info&, bool, bool) { return ""; }
}

namespace Net {
	int width_p = 45, height_p = 28;
	int min_width = 36, min_height = 6;
	int x = 1, y, width = 20, height;
	int b_x, b_y, b_width, b_height, d_graph_height, u_graph_height;
	bool shown = true, redraw = true;
	string box;

	string draw(const net_info&, bool, bool) { return ""; }
}

namespace Proc {
	int width_p = 55, height_p = 68;
	int min_width = 44, min_height = 16;
	int x, y, width = 20, height;
	int start, selected, select_max;
	bool shown = true, redraw = true;
	int selected_pid = 0, selected_depth = 0;
	int scroll_pos;
	string selected_name;
	std::unordered_map<size_t, Draw::Graph> p_graphs;
	std::unordered_map<size_t, int> p_counters;
	Draw::TextEdit filter;
	string box;
	atomic<bool> resized (false);

	int selection(const std::string_view) { return -1; }

	string draw(const vector<proc_info>&, bool, bool) { return ""; }
}

//? ============================================ Draw::calcSizes() ============================================

namespace Draw {
	void calcSizes() {
		atomic_wait(Runner::active);
		Config::unlock();
		auto boxes = Config::getS("shown_boxes");
		auto cpu_bottom = Config::getB("cpu_bottom");
		auto mem_below_net = Config::getB("mem_below_net");
		auto proc_left = Config::getB("proc_left");

		Cpu::box.clear();

		Mem::box.clear();
		Net::box.clear();
		Proc::box.clear();
		Global::clock.clear();
		Global::overlay.clear();
		Runner::pause_output = false;
		Runner::redraw = true;
		if (not (Proc::resized or Global::resized)) {
			Proc::p_counters.clear();
			Proc::p_graphs.clear();
		}
		if (Menu::active) Menu::redraw = true;

		Input::mouse_mappings.clear();

		Cpu::x = Mem::x = Net::x = Proc::x = 1;
		Cpu::y = Mem::y = Net::y = Proc::y = 1;
		Cpu::width = Mem::width = Net::width = Proc::width = 0;
		Cpu::height = Mem::height = Net::height = Proc::height = 0;
		Cpu::redraw = Mem::redraw = Net::redraw = Proc::redraw = true;

		Cpu::shown = boxes.contains("cpu");
	#ifdef GPU_SUPPORT
		Gpu::box.clear();
		Gpu::width = 0;
		Gpu::shown_panels.clear();
		if (Gpu::count > 0) {
			std::istringstream iss(boxes, std::istringstream::in);
			string current;
			while (iss >> current) {
				if (current.starts_with("gpu"))
					Gpu::shown_panels.push_back(current.back()-'0');
			}
		}
		Gpu::shown = Gpu::shown_panels.size();

		Gpu::total_height = 0;
		for (int i = 0; i < Gpu::shown; i++) {
			using namespace Gpu;
			total_height += 4 + gpu_b_height_offsets[shown_panels[i]];
		}
	#endif
		Mem::shown = boxes.contains("mem");
		Net::shown = boxes.contains("net");
		Proc::shown = boxes.contains("proc");

		//* Calculate and draw cpu box outlines
		if (Cpu::shown) {
			using namespace Cpu;
		#ifdef GPU_SUPPORT
			int gpus_extra_height =
				Config::getS("show_gpu_info") == "On" ? Gpu::count
				: Config::getS("show_gpu_info") == "Auto" ? Gpu::count - Gpu::shown
				: 0;
		#endif
            const bool show_temp = (Config::getB("check_temp") and got_sensors);
			width = round((double)Term::width * width_p / 100);
		#ifdef GPU_SUPPORT
			if (Gpu::shown != 0 and not (Mem::shown or Net::shown or Proc::shown)) {
				height = Term::height - Gpu::total_height - gpus_extra_height;
			} else {
				height = max(8, (int)ceil((double)Term::height * (trim(boxes) == "cpu" ? 100 : height_p/(Gpu::shown+1) + (Gpu::shown != 0)*5) / 100));
			}
			if (height <= Term::height-gpus_extra_height) height += gpus_extra_height;
		#else
			height = max(8, (int)ceil((double)Term::height * (trim(boxes) == "cpu" ? 100 : height_p) / 100));
		#endif
			x = 1;
			y = cpu_bottom ? Term::height - height + 1 : 1;

		#ifdef GPU_SUPPORT
			b_columns = max(2, (int)ceil((double)(Shared::coreCount + 1) / (height - gpus_extra_height - 5)));
		#else
			b_columns = max(1, (int)ceil((double)(Shared::coreCount + 1) / (height - 5)));
		#endif
			if (b_columns * (21 + 12 * show_temp) < width - (width / 3)) {
				b_column_size = 2;
				b_width =  max(29, (21 + 12 * show_temp) * b_columns - (b_columns - 1));
			}
			else if (b_columns * (15 + 6 * show_temp) < width - (width / 3)) {
				b_column_size = 1;
				b_width = (15 + 6 * show_temp) * b_columns - (b_columns - 1);
			}
			else if (b_columns * (8 + 6 * show_temp) < width - (width / 3)) {
				b_column_size = 0;
			}
			else {
				b_columns = (width - width / 3) / (8 + 6 * show_temp);
				b_column_size = 0;
			}

			if (b_column_size == 0) b_width = (8 + 6 * show_temp) * b_columns + 1;
		#ifdef GPU_SUPPORT
			b_height = min(height - 2, (int)ceil((double)Shared::coreCount / b_columns) + 4 + gpus_extra_height);
		#else
			b_height = min(height - 2, (int)ceil((double)Shared::coreCount / b_columns) + 4);
		#endif

			b_x = x + width - b_width - 1;
			b_y = y + ceil((double)(height - 2) / 2) - ceil((double)b_height / 2) + 1;

			box = createBox(x, y, width, height, Theme::c("cpu_box"), true, (cpu_bottom ? "" : "cpu"), (cpu_bottom ? "cpu" : ""), 1);

			auto& custom = Config::getS("custom_cpu_name");
			static const bool hasCpuHz = not Cpu::get_cpuHz().empty();
		#ifdef __linux__
			static const bool freq_range = Config::getS("freq_mode") == "range";
		#else
			static const bool freq_range = false;
		#endif
			const string cpu_title = uresize(
					(custom.empty() ? Cpu::cpuName : custom),
					b_width - (Config::getB("show_cpu_freq") and hasCpuHz ? (freq_range ? 24 : 14) : 5)
			);
			box += createBox(b_x, b_y, b_width, b_height, "", false, cpu_title);
		}

	#ifdef GPU_SUPPORT
		//* Calculate and draw gpu box outlines
		if (Gpu::shown != 0) {
			using namespace Gpu;
			x_vec.resize(shown); y_vec.resize(shown);
			b_x_vec.resize(shown); b_y_vec.resize(shown);
			b_height_vec.resize(shown);
			box.resize(shown);
			redraw.resize(shown);
			total_height = 0;
			for (auto i = 0; i < shown; ++i) {
				redraw[i] = true;
				int height = 0;
				width = Term::width;
				if (Cpu::shown)
					if (not (Mem::shown or Net::shown or Proc::shown))
						height = min_height;
					else height = Cpu::height;
				else
					if (not (Mem::shown or Net::shown or Proc::shown))
						height = (Term::height - total_height) / (Gpu::shown - i) + (i == 0) * ((Term::height - total_height) % (Gpu::shown - i));
					else
						height = max(min_height, (int)ceil((double)Term::height * height_p/Gpu::shown / 100));

				b_height_vec[i] = gpu_b_height_offsets[shown_panels[i]] + 2;
				height += (height+Cpu::height == Term::height-1);
				height = max(height, b_height_vec[i] + 2);
				x_vec[i] = 1; y_vec[i] = 1 + total_height + (not Config::getB("cpu_bottom"))*Cpu::shown*Cpu::height;
				box[i] = createBox(x_vec[i], y_vec[i], width, height, Theme::c("cpu_box"), true, std::string("gpu") + (char)(shown_panels[i]+'0'), "", (shown_panels[i]+5)%10);
				b_width = clamp(width/2, min_width, 65);
				total_height += height;

				b_x_vec[i] = x_vec[i] + width - b_width - 1;
				b_y_vec[i] = y_vec[i] + ceil((double)(height - 2 - b_height_vec[i]) / 2) + 1;

				string name = Config::getS(std::string("custom_gpu_name") + (char)(shown_panels[i]+'0'));
				if (name.empty()) name = gpu_names[shown_panels[i]];

				box[i] += createBox(b_x_vec[i], b_y_vec[i], b_width, b_height_vec[i], "", false, name.substr(0, b_width-5));
				b_height_vec[i] = height - 2;
			}
		}
	#endif

		//* Calculate and draw mem box outlines
		if (Mem::shown) {
			using namespace Mem;
			auto show_disks = Config::getB("show_disks");
			auto swap_disk = Config::getB("swap_disk");
			auto mem_graphs = Config::getB("mem_graphs");

			width = round((double)Term::width * (Proc::shown ? width_p : 100) / 100);
		#ifdef GPU_SUPPORT
			height = floor(static_cast<double>(Term::height) * (100 - Net::height_p * Net::shown*4 / ((Gpu::shown != 0 and Cpu::shown) + 4)) / 100) - Cpu::height - Gpu::total_height;
		#else
			height = floor(static_cast<double>(Term::height) * (100 - Cpu::height_p * Cpu::shown - Net::height_p * Net::shown) / 100);
		#endif
			x = (proc_left and Proc::shown) ? Term::width - width + 1: 1;
			if (mem_below_net and Net::shown)
		#ifdef GPU_SUPPORT
				y = Term::height - height + 1 - (cpu_bottom ? Cpu::height : 0);
			else
				y = (cpu_bottom ? 1 : Cpu::height + 1) + Gpu::total_height;
		#else
				y = Term::height - height + 1 - (cpu_bottom ? Cpu::height : 0);
			else
				y = cpu_bottom ? 1 : Cpu::height + 1;
		#endif

			if (show_disks) {
				mem_width = ceil((double)(width - 3) / 2);
				mem_width += mem_width % 2;
				disks_width = width - mem_width - 2;
				divider = x + mem_width;
			}
			else
				mem_width = width - 1;

			item_height = has_swap and not swap_disk ? 6 : 4;
			if (height - (has_swap and not swap_disk ? 3 : 2) > 2 * item_height)
				mem_size = 3;
			else if (mem_width > 25)
				mem_size = 2;
			else
				mem_size = 1;

			mem_meter = max(0, mem_width - (mem_size > 2 ? 7 : 17));
			if (mem_size == 1) mem_meter += 6;

			if (mem_graphs) {
				graph_height = max(1, (int)round((double)((height - (has_swap and not swap_disk ? 2 : 1)) - (mem_size == 3 ? 2 : 1) * item_height) / item_height));
				if (graph_height > 1) mem_meter += 6;
			}
			else
				graph_height = 0;

			if (show_disks) {
				disk_meter = max(-14, width - mem_width - 23);
				if (disks_width < 25) disk_meter += 14;
			}

			box = createBox(x, y, width, height, Theme::c("mem_box"), true, "mem", "", 2);
			box += Mv::to(y, (show_disks ? divider + 2 : x + width - 9)) + Theme::c("mem_box") + Symbols::title_left + (show_disks ? Fx::b : "")
				+ Theme::c("hi_fg") + 'd' + Theme::c("title") + "isks" + Fx::ub + Theme::c("mem_box") + Symbols::title_right;
			Input::mouse_mappings["d"] = {y, (show_disks ? divider + 3 : x + width - 8), 1, 5};
			if (show_disks) {
				box += Mv::to(y, divider) + Symbols::div_up + Mv::to(y + height - 1, divider) + Symbols::div_down + Theme::c("div_line");
				for (auto i : iota(1, height - 1))
					box += Mv::to(y + i, divider) + Symbols::v_line;
			}
		}

		//* Calculate and draw net box outlines
		if (Net::shown) {
			using namespace Net;
			width = round((double)Term::width * (Proc::shown ? width_p : 100) / 100);
		#ifdef GPU_SUPPORT
			height = Term::height - Cpu::height - Gpu::total_height - Mem::height;
		#else
			height = Term::height - Cpu::height - Mem::height;
		#endif
			x = (proc_left and Proc::shown) ? Term::width - width + 1 : 1;
			if (mem_below_net and Mem::shown)
			#ifdef GPU_SUPPORT
				y = (cpu_bottom ? 1 : Cpu::height + 1) + Gpu::total_height;
			#else
				y = cpu_bottom ? 1 : Cpu::height + 1;
			#endif
			else
				y = Term::height - height + 1 - (cpu_bottom ? Cpu::height : 0);

			b_width = (width > 45) ? 27 : 19;
			b_height = (height > 10) ? 9 : height - 2;
			b_x = x + width - b_width - 1;
			b_y = y + ((height - 2) / 2) - b_height / 2 + 1;
			d_graph_height = round((double)(height - 2) / 2);
			u_graph_height = height - 2 - d_graph_height;

			box = createBox(x, y, width, height, Theme::c("net_box"), true, "net", "", 3);
			auto swap_up_down = Config::getB("swap_upload_download");
			if (swap_up_down)
				box += createBox(b_x, b_y, b_width, b_height, "", false, "upload", "download");
			else
				box += createBox(b_x, b_y, b_width, b_height, "", false, "download", "upload");
		}

		//* Calculate and draw proc box outlines
		if (Proc::shown) {
			using namespace Proc;
			width = Term::width - (Mem::shown ? Mem::width : (Net::shown ? Net::width : 0));
		#ifdef GPU_SUPPORT
			height = Term::height - Cpu::height - Gpu::total_height;
		#else
			height = Term::height - Cpu::height;
		#endif
			x = proc_left ? 1 : Term::width - width + 1;
		#ifdef GPU_SUPPORT
			y = ((cpu_bottom and Cpu::shown) ? 1 : Cpu::height + 1) + Gpu::total_height;
		#else
			y = (cpu_bottom and Cpu::shown) ? 1 : Cpu::height + 1;
		#endif
			select_max = height - 3;
			box = createBox(x, y, width, height, Theme::c("proc_box"), true, "proc", "", 4);
		}
	}
}
