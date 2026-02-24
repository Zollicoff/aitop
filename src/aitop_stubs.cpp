/* aitop - AI Tool Usage Monitor
   Stub definitions for btop symbols that are no longer implemented
   by platform-specific collectors but are still referenced by
   btop_draw.cpp, btop_menu.cpp, and btop_shared.cpp.

   These will be removed entirely when the dead btop code is cleaned up (Task 9).
*/

#include "btop_shared.hpp"
#include "btop_draw.hpp"

#include <string>
#include <unordered_map>
#include <vector>

using std::string;
using std::vector;

// ---- Shared namespace stubs ----
namespace Shared {
	long coreCount = 1;
	long page_size = 4096;
	long clk_tck = 100;

	void init() {}
}

// system_uptime is in the Tools namespace (btop_shared.hpp declares it there)
namespace Tools {
	double system_uptime() {
		return 0.0;
	}
}

// ---- Cpu namespace stubs (only symbols NOT defined in btop_draw.cpp) ----
namespace Cpu {
	bool got_sensors{}, cpu_temp_only{}, has_battery{}, supports_watts{};
	string cpuName;
	string cpuHz;
	vector<string> available_fields;
	vector<string> available_sensors;
	std::tuple<int, float, long, string> current_bat;
	std::unordered_map<int, int> core_mapping;

	auto get_cpuHz() -> string { return ""; }
	auto get_core_mapping() -> std::unordered_map<int, int> { return {}; }
	auto get_battery() -> std::tuple<int, float, long, string> { return {}; }

	auto collect(bool) -> cpu_info& {
		static cpu_info info;
		return info;
	}
}

// ---- Mem namespace stubs (only symbols NOT defined in btop_draw.cpp) ----
namespace Mem {
	bool has_swap{};
	int disk_ios{};

	uint64_t get_totalMem() { return 0; }

	auto collect(bool) -> mem_info& {
		static mem_info info;
		return info;
	}
}

// ---- Net namespace stubs (only symbols NOT defined in btop_draw.cpp) ----
namespace Net {
	string selected_iface;
	vector<string> interfaces;
	bool rescale{};
	std::unordered_map<string, uint64_t> graph_max;
	std::unordered_map<string, net_info> current_net;

	auto collect(bool) -> net_info& {
		static net_info info;
		return info;
	}
}

// ---- Proc namespace stubs (only symbols NOT defined in btop_draw.cpp) ----
namespace Proc {
	std::atomic<int> numpids{0};
	std::atomic<int> detailed_pid{0};
	int collapse{}, expand{}, filter_found{}, toggle_children{};

	detail_container detailed;

	auto collect(bool) -> vector<proc_info>& {
		static vector<proc_info> v;
		return v;
	}
}
