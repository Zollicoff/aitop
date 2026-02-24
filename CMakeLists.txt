# SPDX-License-Identifier: Apache-2.0
#
# CMake configuration for btop
#

cmake_minimum_required(VERSION 3.25)

# Disable in-source builds since they would override the Makefile
if("${CMAKE_CURRENT_SOURCE_DIR}" STREQUAL "${CMAKE_CURRENT_BINARY_DIR}")
  message(FATAL_ERROR "In-source builds are not allowed")
endif()

project("btop"
  DESCRIPTION "A monitor of resources"
  HOMEPAGE_URL "https://github.com/aristocratos/btop"
  LANGUAGES CXX
)

include(CheckIPOSupported)
include(CMakeDependentOption)

# Make our Find<Package>.cmake files available
list(APPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/cmake/")

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_COLOR_DIAGNOSTICS ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

option(BTOP_STATIC "Link btop statically" OFF)
option(BTOP_GPU "Enable GPU support (unused in aitop)" OFF)
cmake_dependent_option(BTOP_RSMI_STATIC "Link statically to ROCm SMI" OFF "BTOP_GPU" OFF)

# Enable LTO in release builds by default
if(CMAKE_BUILD_TYPE MATCHES "Rel(ease|WithDebInfo)")
  option(BTOP_LTO "Enable LTO" ON)
else()
  option(BTOP_LTO "Enable LTO" OFF)
endif()

if(BTOP_STATIC AND NOT APPLE)
  # Set this before calling find_package
  set(CMAKE_FIND_LIBRARY_SUFFIXES ".a")
endif()

add_executable(btop src/main.cpp)
add_library(libbtop OBJECT
  src/btop.cpp
  src/btop_cli.cpp
  src/btop_config.cpp
  src/btop_draw.cpp
  src/btop_input.cpp
  src/btop_log.cpp
  src/btop_menu.cpp
  src/btop_shared.cpp
  src/btop_theme.cpp
  src/btop_tools.cpp
  src/aitop_claude.cpp
  src/aitop_codex.cpp
  src/aitop_gemini.cpp
  src/aitop_draw.cpp
  src/aitop_stubs.cpp
)

target_link_libraries(btop libbtop)

if(BTOP_LTO)
  check_ipo_supported()
  set_target_properties(btop PROPERTIES INTERPROCEDURAL_OPTIMIZATION ON)
  set_target_properties(libbtop PROPERTIES INTERPROCEDURAL_OPTIMIZATION ON)
endif()

# Platform-specific collectors removed for aitop
# AI tool parsers (aitop_claude, aitop_codex, aitop_gemini) are used instead

# Generate build info
execute_process(
  COMMAND "git" "rev-parse" "--short" "HEAD"
  WORKING_DIRECTORY "${CMAKE_SOURCE_DIR}"
  OUTPUT_VARIABLE GIT_COMMIT
  OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
set(CONFIGURE_COMMAND
    "cmake -DBTOP_STATIC=${BTOP_STATIC} -DBTOP_GPU=${BTOP_GPU}"
)
get_filename_component(CXX_COMPILER_BASENAME "${CMAKE_CXX_COMPILER}" NAME)
set(COMPILER "${CXX_COMPILER_BASENAME}")
set(COMPILER_VERSION "${CMAKE_CXX_COMPILER_VERSION}")
configure_file(${CMAKE_CURRENT_SOURCE_DIR}/src/config.h.in ${CMAKE_CURRENT_BINARY_DIR}/config.h @ONLY IMMEDIATE)
set(CMAKE_INCLUDE_CURRENT_DIR ON)

include(CheckCXXSourceCompiles)
include(CheckCXXSymbolExists)

check_cxx_source_compiles(
  "
  #include <version>
  #if __cpp_lib_optional < 202110L
  #error \"Missing __cpp_lib_optional < 202110L\"
  #endif
  int main() { return 0; }
  "
  HAS_CXX_OPTIONAL_MONADS
)
if(NOT HAS_CXX_OPTIONAL_MONADS)
  message(FATAL_ERROR "The compiler doesn't support std::optional::and_then")
endif()
check_cxx_symbol_exists("__cpp_lib_expected" "version" HAS_CXX_EXPECTED)
if(NOT HAS_CXX_EXPECTED)
  message(FATAL_ERROR "The compiler doesn't support std::expected")
endif()
check_cxx_symbol_exists("__cpp_lib_ranges" "version" HAS_CXX_RANGES)
if(NOT HAS_CXX_RANGES)
  message(FATAL_ERROR "The compiler doesn't support std::ranges")
endif()
# ranges::to check removed for aitop (replaced with iterator-based alternatives)
check_cxx_symbol_exists("__cpp_lib_string_contains" "version" HAS_CXX_STRING_CONTAINS)
if(NOT HAS_CXX_STRING_CONTAINS)
  message(FATAL_ERROR "The compiler doesn't support std::string::contains")
endif()

target_compile_options(libbtop PUBLIC -Wall -Wextra -Wpedantic)

include(CheckCXXCompilerFlag)

if(NOT APPLE)
  target_compile_options(libbtop PUBLIC -fstack-clash-protection)
endif()
check_cxx_compiler_flag(-fstack-protector HAS_FSTACK_PROTECTOR)
if(HAS_FSTACK_PROTECTOR)
  target_compile_options(libbtop PUBLIC -fstack-protector)
endif()
check_cxx_compiler_flag(-fcf-protection HAS_FCF_PROTECTION)
if(HAS_FCF_PROTECTION)
  target_compile_options(libbtop PUBLIC -fcf-protection)
endif()

target_compile_definitions(libbtop PUBLIC
  FMT_HEADER_ONLY
  _FILE_OFFSET_BITS=64
  $<$<CONFIG:Debug>:_GLIBCXX_ASSERTIONS _LIBCPP_HARDENING_MODE=_LIBCPP_HARDENING_MODE_DEBUG>
)

target_include_directories(libbtop SYSTEM PUBLIC include)

# Enable pthreads
set(THREADS_PREFER_PTHREAD_FLAG ON)
find_package(Threads REQUIRED)
target_link_libraries(libbtop Threads::Threads)

# GPU support removed for aitop

if(BTOP_STATIC)
  target_compile_definitions(libbtop PUBLIC STATIC_BUILD)
  target_link_options(libbtop PUBLIC -static LINKER:--fatal-warnings)
endif()

# Apple GPU support removed for aitop

# Platform-dependent library linking removed for aitop

# Check if lowdown is installed
find_program(LOWDOWN_EXECUTABLE lowdown)

if(LOWDOWN_EXECUTABLE)
  # Custom target to compile Markdown to man page using lowdown
  add_custom_command(
    OUTPUT btop.1
    COMMAND lowdown -s -T man -o btop.1 ${CMAKE_CURRENT_SOURCE_DIR}/manpage.md
    DEPENDS ${CMAKE_CURRENT_SOURCE_DIR}/manpage.md
    VERBATIM
  )
  add_custom_target(generate_manpage ALL DEPENDS ${CMAKE_CURRENT_BINARY_DIR}/btop.1)
  # Install the man page
  install(FILES "${CMAKE_CURRENT_BINARY_DIR}/btop.1" DESTINATION "share/man/man1")
else()
  message(STATUS "Command 'lowdown' not found: skipping generating man page btop.1")
endif()

install(TARGETS btop RUNTIME)
install(FILES "btop.desktop" DESTINATION "share/applications")
install(FILES "Img/icon.png" DESTINATION "share/icons/hicolor/48x48/apps" RENAME "btop.png")
install(FILES "Img/icon.svg" DESTINATION "share/icons/hicolor/scalable/apps" RENAME "btop.svg")
install(DIRECTORY "themes" DESTINATION "share/btop")

include(CTest)
if(BUILD_TESTING)
  add_subdirectory(tests)
endif()
