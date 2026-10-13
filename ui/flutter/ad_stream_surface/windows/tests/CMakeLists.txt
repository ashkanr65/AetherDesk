cmake_minimum_required(VERSION 3.14)
project(aetherdesk_desktop_tests LANGUAGES CXX)
enable_testing()
add_executable(desktop_platform_test ../desktop_platform_test.cpp)
target_compile_features(desktop_platform_test PRIVATE cxx_std_17)
if(MSVC)
  target_compile_options(desktop_platform_test PRIVATE /W4 /WX /EHsc)
else()
  target_compile_options(desktop_platform_test PRIVATE -Wall -Wextra -Werror)
endif()
add_test(NAME desktop_platform COMMAND desktop_platform_test)

if(WIN32)
  add_executable(desktop_windows_test ../desktop_windows_test.cpp)
  target_compile_features(desktop_windows_test PRIVATE cxx_std_17)
  target_compile_definitions(desktop_windows_test PRIVATE UNICODE _UNICODE)
  target_link_libraries(desktop_windows_test PRIVATE user32 gdi32)
  if(MSVC)
    target_compile_options(desktop_windows_test PRIVATE /W4 /WX /permissive- /EHsc)
  endif()
  add_test(NAME desktop_windows COMMAND desktop_windows_test)
endif()

# Optional engine-free integration test of the production plugin. Point this at
# <flutter-sdk>/bin/cache/artifacts/engine/windows-x64 (the public SDK artifacts).
set(FLUTTER_WINDOWS_ARTIFACTS "" CACHE PATH "Flutter Windows SDK artifacts")
# The beginner kit and CI put Flutter on PATH before this suite. Discover its
# headers automatically so the shutdown regression runs in their normal checks.
if(WIN32 AND NOT FLUTTER_WINDOWS_ARTIFACTS)
  # CI supplies a native Windows path; source lists require CMake separators.
  file(TO_CMAKE_PATH "$ENV{FLUTTER_ROOT}" AETHERDESK_FLUTTER_ROOT)
  if(NOT EXISTS "${AETHERDESK_FLUTTER_ROOT}/bin/flutter.bat")
    find_program(AETHERDESK_FLUTTER_TOOL NAMES flutter.bat flutter)
    if(AETHERDESK_FLUTTER_TOOL)
      get_filename_component(AETHERDESK_FLUTTER_BIN "${AETHERDESK_FLUTTER_TOOL}" DIRECTORY)
      get_filename_component(AETHERDESK_FLUTTER_ROOT "${AETHERDESK_FLUTTER_BIN}" DIRECTORY)
    endif()
  endif()
  set(AETHERDESK_FLUTTER_ARTIFACTS "${AETHERDESK_FLUTTER_ROOT}/bin/cache/artifacts/engine/windows-x64")
  if(EXISTS "${AETHERDESK_FLUTTER_ARTIFACTS}/cpp_client_wrapper/core_implementations.cc")
    set(FLUTTER_WINDOWS_ARTIFACTS "${AETHERDESK_FLUTTER_ARTIFACTS}")
  endif()
endif()
if(WIN32 AND FLUTTER_WINDOWS_ARTIFACTS)
  # Also normalize an explicit -DFLUTTER_WINDOWS_ARTIFACTS override.
  file(TO_CMAKE_PATH "${FLUTTER_WINDOWS_ARTIFACTS}" FLUTTER_WINDOWS_ARTIFACTS)
  set(FLUTTER_WRAPPER "${FLUTTER_WINDOWS_ARTIFACTS}/cpp_client_wrapper")
  add_executable(stream_surface_shutdown_test
    ../stream_surface_shutdown_test.cpp
    ../ad_stream_surface_plugin.cpp
    "${FLUTTER_WRAPPER}/core_implementations.cc"
    "${FLUTTER_WRAPPER}/standard_codec.cc"
    "${FLUTTER_WRAPPER}/plugin_registrar.cc")
  target_compile_features(stream_surface_shutdown_test PRIVATE cxx_std_17)
  target_compile_definitions(stream_surface_shutdown_test PRIVATE
    FLUTTER_DESKTOP_LIBRARY FLUTTER_PLUGIN_IMPL UNICODE _UNICODE)
  target_include_directories(stream_surface_shutdown_test PRIVATE
    "${FLUTTER_WINDOWS_ARTIFACTS}" "${FLUTTER_WRAPPER}/include")
  target_link_libraries(stream_surface_shutdown_test PRIVATE user32 gdi32)
  if(MSVC)
    target_compile_options(stream_surface_shutdown_test PRIVATE /W4 /WX /permissive- /EHsc)
  endif()
  add_test(NAME stream_surface_shutdown COMMAND stream_surface_shutdown_test)
  message(STATUS "Native editor shutdown regression enabled")
elseif(WIN32)
  message(STATUS "Flutter SDK not found; set FLUTTER_WINDOWS_ARTIFACTS to enable the editor shutdown regression")
endif()
