# SHELLOOP as a VST3 instrument.
#
# The CLAP plugin (clap-plugin/, a Rust static library) is linked straight into
# a clap-wrapper VST3 module, so the result is one self-contained .vst3 that
# does not need shelloop.clap installed next to it.
#
# Build (from the repository root):
#   cargo build --release -p shelloop-clap
#   cmake -S vst3 -B vst3/build -DCMAKE_BUILD_TYPE=Release
#   cmake --build vst3/build --config Release
cmake_minimum_required(VERSION 3.21)
project(shelloop_vst3 LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
# The wrapper's static libraries end up inside a shared module.
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# Fetch the VST3 SDK and the CLAP headers on demand.
set(CLAP_WRAPPER_DOWNLOAD_DEPENDENCIES TRUE CACHE BOOL "" FORCE)
set(CLAP_WRAPPER_DONT_ADD_TARGETS TRUE CACHE BOOL "" FORCE)

include(FetchContent)
FetchContent_Declare(clap-wrapper
  GIT_REPOSITORY https://github.com/free-audio/clap-wrapper.git
  GIT_TAG v0.9.1
  GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(clap-wrapper)

# The Rust static library built by `cargo build --release -p shelloop-clap`.
get_filename_component(REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/.." ABSOLUTE)
set(RUST_TARGET_DIR "${REPO_ROOT}/target/release" CACHE PATH "Cargo release output directory")
if(WIN32)
  set(RUST_STATICLIB "${RUST_TARGET_DIR}/shelloop_clap.lib")
else()
  set(RUST_STATICLIB "${RUST_TARGET_DIR}/libshelloop_clap.a")
endif()
if(NOT EXISTS "${RUST_STATICLIB}")
  message(FATAL_ERROR "Missing ${RUST_STATICLIB}. Run: cargo build --release -p shelloop-clap")
endif()

add_library(shelloop_vst3 MODULE)
target_add_vst3_wrapper(
  TARGET shelloop_vst3
  OUTPUT_NAME "SHELLOOP"
  SUPPORTS_ALL_NOTE_EXPRESSIONS FALSE)

if(WIN32)
  # Native libraries a Rust static library needs on Windows, plus an explicit
  # export so the wrapper finds `clap_entry` in its own module.
  target_link_libraries(shelloop_vst3 PRIVATE
    "${RUST_STATICLIB}" ws2_32 userenv ntdll bcrypt advapi32 kernel32 shell32 ole32 oleaut32 dbghelp)
  target_link_options(shelloop_vst3 PRIVATE "/EXPORT:clap_entry,DATA" "/INCLUDE:clap_entry")
else()
  # Keep every object of the archive and export `clap_entry` dynamically.
  target_link_libraries(shelloop_vst3 PRIVATE
    -Wl,--whole-archive "${RUST_STATICLIB}" -Wl,--no-whole-archive pthread dl m)
  target_link_options(shelloop_vst3 PRIVATE "-Wl,--export-dynamic-symbol=clap_entry")
endif()
