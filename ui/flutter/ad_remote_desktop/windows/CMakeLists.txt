cmake_minimum_required(VERSION 3.14)

project(ad_remote_desktop LANGUAGES CXX)

set(BINARY_NAME "ad_remote_desktop")

cmake_policy(SET CMP0063 NEW)

set(CMAKE_INSTALL_RPATH "")

# Configure build options.
if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
  set(CMAKE_BUILD_TYPE "Debug" CACHE
    STRING "Flutter build mode" FORCE)
  set_property(CACHE CMAKE_BUILD_TYPE PROPERTY STRINGS
    "Debug" "Profile" "Release")
endif()

# Use Unicode for Win32 APIs.
add_definitions(-DUNICODE -D_UNICODE)

# Compilation settings that should be applied to most targets.
function(APPLY_STANDARD_SETTINGS TARGET)
  target_compile_features(${TARGET} PRIVATE cxx_std_17)
  target_compile_options(${TARGET} PRIVATE /W4 /WX /wd"4100")
  target_compile_options(${TARGET} PRIVATE /EHsc)
  target_compile_definitions(${TARGET} PRIVATE "_HAS_EXCEPTIONS=0")
  target_compile_definitions(${TARGET} PRIVATE $<$<CONFIG:Debug>:_DEBUG>)
endfunction()

set(FLUTTER_MANAGED_DIR "${CMAKE_CURRENT_SOURCE_DIR}/flutter")

# Flutter library and tool build rules.
add_subdirectory(${FLUTTER_MANAGED_DIR})

# Application build
add_subdirectory("runner")

# Rust FFI Library
find_library(RUST_LIB ad_ssh_ffi
  HINTS "${CMAKE_CURRENT_SOURCE_DIR}/lib"
  NO_DEFAULT_PATH
)

if(RUST_LIB)
  # Add Rust library as prebuilt
  add_library(rust_ffi SHARED IMPORTED)
  set_target_properties(rust_ffi PROPERTIES
    IMPORTED_LOCATION "${CMAKE_CURRENT_SOURCE_DIR}/lib/ad_ssh_ffi.dll"
    IMPORTED_IMPLIB "${CMAKE_CURRENT_SOURCE_DIR}/lib/ad_ssh_ffi.lib"
  )
  
  # Link Rust library to the application
  target_link_libraries(${BINARY_NAME} PRIVATE rust_ffi)
  
  # Add include directory for Rust FFI headers
  target_include_directories(${BINARY_NAME} PRIVATE 
    "${CMAKE_CURRENT_SOURCE_DIR}/../src/include"
  )
else()
  message(WARNING "Rust FFI library not found. The application may not function correctly.")
endif()

# Generated plugin build rules, which manage building the plugins and adding
# them to the application.
include(flutter/generated_plugins.cmake)


# === Installation ===
# By default, "installing" just makes a relocatable bundle in the build
# directory.
install(DIRECTORY "${PROJECT_BUILD_DIR}/${BINARY_NAME}.exe"
  DESTINATION "${CMAKE_INSTALL_PREFIX}"
  COMPONENT Runtime)
