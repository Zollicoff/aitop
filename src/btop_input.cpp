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

#include <limits>
#include <ranges>
#include <vector>
#include <thread>
#include <mutex>
#include <fmt/format.h>
#include <signal.h>
#include <sys/select.h>
#include <utility>
#include <cmath>

#include "btop_input.hpp"
#include "btop_tools.hpp"
#include "btop_config.hpp"
#include "btop_shared.hpp"
#include "btop_menu.hpp"
#include "btop_draw.hpp"
#include "aitop_draw.hpp"

using namespace Tools;
using namespace std::literals; // for operator""s
namespace rng = std::ranges;

namespace Input {

	//* Map for translating key codes to readable values
	const std::unordered_map<string, string> Key_escapes = {
		{"\033",	"escape"},
		{"\x12",	"ctrl_r"},
		{"\n",		"enter"},
		{" ",		"space"},
		{"\x7f",	"backspace"},
		{"\x08",	"backspace"},
		{"[A", 		"up"},
		{"OA",		"up"},
		{"[B", 		"down"},
		{"OB",		"down"},
		{"[D", 		"left"},
		{"OD",		"left"},
		{"[C", 		"right"},
		{"OC",		"right"},
		{"[2~",		"insert"},
		{"[4h",		"insert"},
		{"[3~",		"delete"},
		{"[P",		"delete"},
		{"[H",		"home"},
		{"[1~",		"home"},
		{"[F",		"end"},
		{"[4~",		"end"},
		{"[5~",		"page_up"},
		{"[6~",		"page_down"},
		{"\t",		"tab"},
		{"[Z",		"shift_tab"},
		{"OP",		"f1"},
		{"OQ",		"f2"},
		{"OR",		"f3"},
		{"OS",		"f4"},
		{"[15~",	"f5"},
		{"[17~",	"f6"},
		{"[18~",	"f7"},
		{"[19~",	"f8"},
		{"[20~",	"f9"},
		{"[21~",	"f10"},
		{"[23~",	"f11"},
		{"[24~",	"f12"}
	};

	sigset_t signal_mask;
	std::atomic<bool> polling (false);
	array<int, 2> mouse_pos;
	std::unordered_map<string, Mouse_loc> mouse_mappings;
	bool dragging_scroll;

	deque<string> history(50, "");
	string old_filter;
	string input;

	bool poll(const uint64_t timeout) {
		atomic_lock lck(polling);
		fd_set fds;
		FD_ZERO(&fds);
		FD_SET(STDIN_FILENO, &fds);
		struct timespec wait;
		struct timespec *waitptr = nullptr;

		if(timeout != std::numeric_limits<uint64_t>::max()) {
			wait.tv_sec = timeout / 1000;
			wait.tv_nsec = (timeout % 1000) * 1000000;
			waitptr = &wait;
		}

		if(pselect(STDIN_FILENO + 1, &fds, nullptr, nullptr, waitptr, &signal_mask) > 0) {
			input.clear();
			char buf[1024];
			ssize_t count = 0;
			while((count = read(STDIN_FILENO, buf, sizeof(buf))) > 0) {
				input.append(std::string_view(buf, count));
			}

			return true;
		}

		return false;
	}

	string get() {
		string key = input;
		if (not key.empty()) {
			//? Remove escape code prefix if present
			if (key.length() > 1 and key.at(0) == Fx::e.at(0)) {
				key.erase(0, 1);
			}
			
			//? Detect if input is a mouse event.
			if (key.starts_with("[<")) {
				std::string_view key_view = key;
				string mouse_event;
				if (key_view.starts_with("[<0;") and key_view.find('M') != std::string_view::npos) {
					mouse_event = "mouse_click";
					key_view.remove_prefix(4);
				}
				else if (key_view.starts_with("[<32;")) {
					mouse_event = "mouse_drag";
					key_view.remove_prefix(5);
				}
				else if (key_view.starts_with("[<0;") and key_view.ends_with('m')) {
					mouse_event = "mouse_release";
					key_view.remove_prefix(4);
				}
				else if (key_view.starts_with("[<64;")) {
					mouse_event = "mouse_scroll_up";
					key_view.remove_prefix(5);
				}
				else if (key_view.starts_with("[<65;")) {
					mouse_event = "mouse_scroll_down";
					key_view.remove_prefix(5);
				}
				else
					key.clear();

				if (Config::getB("proc_filtering")) {
					if (mouse_event == "mouse_click") return mouse_event;
					else return "";
				}

				//? Get column and line position of mouse and check for any actions mapped to current position
				if (not key.empty()) {
					try {
						const auto delim = key_view.find(';');
						mouse_pos[0] = stoi((string)key_view.substr(0, delim));
						mouse_pos[1] = stoi((string)key_view.substr(delim + 1, key_view.find('M', delim)));
					}
					catch (const std::invalid_argument&) { mouse_event.clear(); }
					catch (const std::out_of_range&) { mouse_event.clear(); }

					key = mouse_event;

					if (key == "mouse_click" or key == "mouse_drag") {
						const auto& [col, line] = mouse_pos;

						for (const auto& [mapped_key, pos] : (Menu::active ? Menu::mouse_mappings : mouse_mappings)) {
							if (col >= pos.col and col < pos.col + pos.width and line >= pos.line and line < pos.line + pos.height) {
								key = mapped_key;
								break;
							}
						}
					}
				}

			}
			else if (auto it = Key_escapes.find(key); it != Key_escapes.end())
				key = it->second;
			else if (ulen(key) > 1)
				key.clear();

			if (not key.empty()) {
				history.push_back(key);
				history.pop_front();
			}
		}
		return key;
	}

	string wait() {
		while(not poll(std::numeric_limits<uint64_t>::max())) {}
		return get();
	}

	void interrupt() {
		kill(getpid(), SIGUSR1);
	}

	void clear() {
		// do not need it, actually
	}

	void process(const std::string_view key) {
		if (key.empty()) return;
		try {
			auto vim_keys = Config::getB("vim_keys");
			auto help_key = (vim_keys ? "H" : "h");

			//? Global input actions
			if (key == "q") {
				clean_quit(0);
			}
			else if (is_in(key, "escape", "m")) {
				Menu::show(Menu::Menus::Main);
				return;
			}
			else if (is_in(key, "f1", "?", help_key)) {
				Menu::show(Menu::Menus::Help);
				return;
			}
			else if (is_in(key, "f2", "o")) {
				Menu::show(Menu::Menus::Options);
				return;
			}
			else if (is_in(key, "ctrl_r")) {
				kill(getpid(), SIGUSR2);
				return;
			}

			//? aitop band navigation
			else if (key == "tab") {
				AiDraw::selection("tab");
				Runner::run("all", true);
			}
			else if (key == "up" or (vim_keys and key == "k")) {
				AiDraw::selection("up");
				Runner::run("all", true);
			}
			else if (key == "down" or (vim_keys and key == "j")) {
				AiDraw::selection("down");
				Runner::run("all", true);
			}
			else if (key == "page_up") {
				AiDraw::selection("page_up");
				Runner::run("all", true);
			}
			else if (key == "page_down") {
				AiDraw::selection("page_down");
				Runner::run("all", true);
			}
		}

		catch (const std::exception& e) {
			throw std::runtime_error { fmt::format(R"(Input::process("{}"))", e.what()) };
		}
	}
}
