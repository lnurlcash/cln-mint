#!/bin/bash
set -euo pipefail
# Boost's headers for Bitcoin Core's CMake, for any target: Ubuntu's own
# BoostConfig sits under the HOST's multiarch dir, which a cross build skips.
# Only boost/ is exposed, never /usr/include itself, whose glibc headers
# would shadow the cross toolchain's.
dir="${1:?usage: tools/boost-shim.sh DIR}"
mkdir -p "$dir/include"
ln -sfn /usr/include/boost "$dir/include/boost"
cat > "$dir/BoostConfig.cmake" <<CMAKE
set(Boost_INCLUDE_DIR "$dir/include")
if(NOT TARGET Boost::headers)
  add_library(Boost::headers INTERFACE IMPORTED)
  set_target_properties(Boost::headers PROPERTIES INTERFACE_INCLUDE_DIRECTORIES "\${Boost_INCLUDE_DIR}")
endif()
set(Boost_FOUND TRUE)
CMAKE
version=$(sed -n 's/^#define BOOST_LIB_VERSION "\(.*\)"/\1/p' /usr/include/boost/version.hpp | tr _ .)
cat > "$dir/BoostConfigVersion.cmake" <<CMAKE
set(PACKAGE_VERSION "$version")
if(PACKAGE_FIND_VERSION VERSION_GREATER PACKAGE_VERSION)
  set(PACKAGE_VERSION_COMPATIBLE FALSE)
else()
  set(PACKAGE_VERSION_COMPATIBLE TRUE)
endif()
CMAKE
