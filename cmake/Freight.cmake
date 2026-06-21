# Freight.cmake — consume Freight packages from a CMake project.
#
# This is the mirror of `freight migrate`: instead of converting a CMake project
# to Freight, it lets an existing CMake build pull in a Freight library and link
# it like any other target. It works by calling the `freight` CLI to build and
# install the package into the build tree, then importing the result (preferring
# the emitted pkg-config `.pc`, with a direct fallback).
#
# Usage:
#
#   list(APPEND CMAKE_MODULE_PATH "/path/to/freight/cmake")
#   include(Freight)
#
#   # Build + import a local Freight project:
#   freight_dependency(mylib SOURCE_DIR ${CMAKE_SOURCE_DIR}/../mylib)
#
#   # Or import an already-installed Freight package (found via pkg-config):
#   freight_dependency(mylib)
#
#   target_link_libraries(myapp PRIVATE freight::mylib)
#
# Enable Freight features on a built dependency:
#
#   freight_dependency(mylib SOURCE_DIR ../mylib FEATURES tls zlib NO_DEFAULT_FEATURES)
#
# Options for freight_dependency(<name> ...):
#   SOURCE_DIR <dir>      Path to a Freight project to build + install on the fly.
#   PREFIX <dir>          Install prefix (default: ${CMAKE_BINARY_DIR}/_freight/<name>).
#   RELEASE | DEBUG       Build profile to install (default: matches CMAKE_BUILD_TYPE,
#                         else RELEASE).
#   FEATURES <f...>       Freight features to activate (passed to `freight install
#                         --features`). Only applies when building (SOURCE_DIR).
#   NO_DEFAULT_FEATURES   Pass `--no-default-features` to the build.
#   STATIC | SHARED       Link preference for the manual-import fallback.
#   ALIAS <target>        Extra alias to create in addition to freight::<name>.
#   REQUIRED              Fail (FATAL_ERROR) if the package cannot be imported.

if(DEFINED _FREIGHT_CMAKE_INCLUDED)
  return()
endif()
set(_FREIGHT_CMAKE_INCLUDED TRUE)

# Locate the freight CLI. Override with -DFREIGHT_EXECUTABLE=/path/to/freight.
if(NOT FREIGHT_EXECUTABLE)
  find_program(FREIGHT_EXECUTABLE
    NAMES freight
    HINTS ENV FREIGHT_HOME "$ENV{HOME}/.cargo/bin" "$ENV{HOME}/.local/bin"
    DOC "Path to the freight build tool")
endif()

# Build + install a Freight project into <prefix> at configure time.
# `extra` is an optional ;-list of additional `freight install` arguments
# (e.g. feature flags).
function(_freight_build_install src prefix profile extra)
  if(NOT FREIGHT_EXECUTABLE)
    message(FATAL_ERROR "freight_dependency: the `freight` executable was not found. "
      "Install it or set -DFREIGHT_EXECUTABLE=/path/to/freight.")
  endif()
  set(_args install --prefix "${prefix}")
  if(profile STREQUAL "release")
    list(APPEND _args --release)
  endif()
  if(extra)
    list(APPEND _args ${extra})
  endif()
  message(STATUS "Freight: ${FREIGHT_EXECUTABLE} ${_args} (in ${src})")
  execute_process(
    COMMAND ${FREIGHT_EXECUTABLE} ${_args}
    WORKING_DIRECTORY "${src}"
    RESULT_VARIABLE _rc
    OUTPUT_VARIABLE _out
    ERROR_VARIABLE _err)
  if(NOT _rc EQUAL 0)
    message(FATAL_ERROR "freight install failed (${_rc}) for ${src}:\n${_out}\n${_err}")
  endif()
endfunction()

# Import an installed prefix as freight::<name>, preferring its .pc file.
function(_freight_import name prefix want_static required out_found)
  set(${out_found} FALSE PARENT_SCOPE)

  # 1. Preferred path: consume the emitted pkg-config descriptor.
  find_package(PkgConfig QUIET)
  if(PkgConfig_FOUND AND EXISTS "${prefix}/lib/pkgconfig/${name}.pc")
    set(_saved_pcp "$ENV{PKG_CONFIG_PATH}")
    if(_saved_pcp STREQUAL "")
      set(ENV{PKG_CONFIG_PATH} "${prefix}/lib/pkgconfig")
    else()
      set(ENV{PKG_CONFIG_PATH} "${prefix}/lib/pkgconfig:${_saved_pcp}")
    endif()
    pkg_check_modules(FD_${name} QUIET IMPORTED_TARGET GLOBAL ${name})
    set(ENV{PKG_CONFIG_PATH} "${_saved_pcp}")
    if(FD_${name}_FOUND AND TARGET PkgConfig::FD_${name})
      if(NOT TARGET freight::${name})
        add_library(freight::${name} ALIAS PkgConfig::FD_${name})
      endif()
      set(${out_found} TRUE PARENT_SCOPE)
      return()
    endif()
  endif()

  # 2. Fallback: import the installed artifact directly from the known layout.
  set(_inc "${prefix}/include")
  set(_libdir "${prefix}/lib")
  if(want_static)
    set(_names "lib${name}.a" "${name}.lib")
    set(_kind STATIC)
  else()
    set(_names "lib${name}.so" "lib${name}.dylib" "${name}.dll" "lib${name}.a" "${name}.lib")
    set(_kind UNKNOWN)
  endif()
  set(_libpath "")
  foreach(_n ${_names})
    if(EXISTS "${_libdir}/${_n}")
      set(_libpath "${_libdir}/${_n}")
      break()
    endif()
  endforeach()

  if(_libpath STREQUAL "")
    # Header-only: an interface target with just the include dir.
    if(EXISTS "${_inc}")
      if(NOT TARGET freight::${name})
        add_library(freight::${name} INTERFACE IMPORTED GLOBAL)
        set_target_properties(freight::${name} PROPERTIES
          INTERFACE_INCLUDE_DIRECTORIES "${_inc}")
      endif()
      set(${out_found} TRUE PARENT_SCOPE)
      return()
    endif()
    if(required)
      message(FATAL_ERROR "freight_dependency(${name}): no library or headers found under ${prefix}")
    endif()
    return()
  endif()

  if(NOT TARGET freight::${name})
    add_library(freight::${name} ${_kind} IMPORTED GLOBAL)
    set_target_properties(freight::${name} PROPERTIES
      IMPORTED_LOCATION "${_libpath}"
      INTERFACE_INCLUDE_DIRECTORIES "${_inc}")
  endif()
  set(${out_found} TRUE PARENT_SCOPE)
endfunction()

function(freight_dependency name)
  cmake_parse_arguments(FD
    "STATIC;SHARED;RELEASE;DEBUG;REQUIRED;NO_DEFAULT_FEATURES"
    "SOURCE_DIR;PREFIX;ALIAS"
    "FEATURES"
    ${ARGN})

  # Resolve profile.
  if(FD_RELEASE)
    set(_profile release)
  elseif(FD_DEBUG)
    set(_profile debug)
  elseif(CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(_profile debug)
  else()
    set(_profile release)
  endif()

  # Resolve prefix.
  if(FD_PREFIX)
    set(_prefix "${FD_PREFIX}")
  else()
    set(_prefix "${CMAKE_BINARY_DIR}/_freight/${name}")
  endif()

  # Feature flags forwarded to `freight install` (only meaningful when building).
  set(_extra "")
  if(FD_FEATURES)
    string(REPLACE ";" "," _feat "${FD_FEATURES}")
    list(APPEND _extra --features ${_feat})
  endif()
  if(FD_NO_DEFAULT_FEATURES)
    list(APPEND _extra --no-default-features)
  endif()

  # Build + install from source when a project dir is given.
  if(FD_SOURCE_DIR)
    _freight_build_install("${FD_SOURCE_DIR}" "${_prefix}" "${_profile}" "${_extra}")
    _freight_import("${name}" "${_prefix}" "${FD_STATIC}" "${FD_REQUIRED}" _found)
  else()
    if(_extra)
      message(WARNING "freight_dependency(${name}): FEATURES/NO_DEFAULT_FEATURES are "
        "ignored without SOURCE_DIR (the installed artifact is already built).")
    endif()
    # No source: try the build-tree prefix first, then the system pkg-config DB.
    _freight_import("${name}" "${_prefix}" "${FD_STATIC}" FALSE _found)
    if(NOT _found)
      find_package(PkgConfig QUIET)
      if(PkgConfig_FOUND)
        pkg_check_modules(FD_${name} QUIET IMPORTED_TARGET GLOBAL ${name})
        if(FD_${name}_FOUND AND TARGET PkgConfig::FD_${name})
          if(NOT TARGET freight::${name})
            add_library(freight::${name} ALIAS PkgConfig::FD_${name})
          endif()
          set(_found TRUE)
        endif()
      endif()
    endif()
  endif()

  if(NOT _found)
    if(FD_REQUIRED)
      message(FATAL_ERROR "freight_dependency(${name}): could not import the package. "
        "Pass SOURCE_DIR to build it, or install it so pkg-config can find ${name}.pc.")
    else()
      message(WARNING "freight_dependency(${name}): package not found; target freight::${name} not created.")
      return()
    endif()
  endif()

  if(FD_ALIAS AND TARGET freight::${name} AND NOT TARGET ${FD_ALIAS})
    add_library(${FD_ALIAS} ALIAS freight::${name})
  endif()

  message(STATUS "Freight: imported freight::${name} from ${_prefix}")
endfunction()
