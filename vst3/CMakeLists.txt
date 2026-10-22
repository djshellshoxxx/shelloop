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
if(APPLE AND NOT DEFINED CMAKE_OSX_DEPLOYMENT_TARGET)
  set(CMAKE_OSX_DEPLOYMENT_TARGET "11.0" CACHE STRING "Minimum macOS version")
endif()
project(shelloop_vst3 LANGUAGES C CXX)
if(APPLE)
  enable_language(OBJC OBJCXX)
endif()

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

# clap-wrapper adds -Werror to its own compile options. Its vendored sources
# (SHA-1, bundled fmt) trip warnings that newer GCC (15.x on the ubuntu-latest
# runner) reports as errors, so the dependency build stops. Remove -Werror from
# the wrapper's interface options only. Warnings still print (-Wall -Wextra).
# SHELLOOP's own warning flags are not touched.
if(TARGET clap-wrapper-compile-options)
  get_target_property(clap_wrapper_options clap-wrapper-compile-options INTERFACE_COMPILE_OPTIONS)
  if(clap_wrapper_options)
    list(REMOVE_ITEM clap_wrapper_options "-Werror")
    set_target_properties(clap-wrapper-compile-options PROPERTIES
      INTERFACE_COMPILE_OPTIONS "${clap_wrapper_options}")
  endif()
endif()

# clap-wrapper forces the static MSVC runtime (/MT) for the targets it creates
# itself, while this module and the Rust plugin use the DLL runtime (/MD), as
# the CLAP build does. Mixing them fails to link, so put every wrapper-owned
# library on /MD (a no-op outside MSVC).
if(MSVC)
  foreach(wrapper_lib clap-wrapper-shared-detail)
    if(TARGET ${wrapper_lib})
      set_target_properties(${wrapper_lib} PROPERTIES MSVC_RUNTIME_LIBRARY "MultiThreadedDLL")
    endif()
  endforeach()
endif()

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
elseif(APPLE)
  # Mach-O uses force_load rather than ELF whole-archive/export-dynamic-symbol.
  # Keep the Rust CLAP entry visible so the wrapper can discover itself.
  target_link_libraries(shelloop_vst3 PRIVATE "${RUST_STATICLIB}"
    "-framework Security" "-framework CoreFoundation")
  target_link_options(shelloop_vst3 PRIVATE
    "-Wl,-force_load,${RUST_STATICLIB}")
else()
  # Keep every object of the archive and export `clap_entry` dynamically.
  target_link_libraries(shelloop_vst3 PRIVATE
    -Wl,--whole-archive "${RUST_STATICLIB}" -Wl,--no-whole-archive pthread dl m)
  target_link_options(shelloop_vst3 PRIVATE "-Wl,--export-dynamic-symbol=clap_entry")
endif()
