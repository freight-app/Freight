# CMake generated Testfile for 
# Source directory: /home/max/Freight/examples/with-external-deps/.deps/json/tests/cmake_fetch_content
# Build directory: /home/max/Freight/examples/with-external-deps/.deps/json/.freight-build/tests/cmake_fetch_content
# 
# This file includes the relevant testing commands required for 
# testing this directory and lists subdirectories to be tested as well.
add_test(cmake_fetch_content_configure "/usr/bin/cmake" "-G" "Unix Makefiles" "-DCMAKE_CXX_COMPILER=/usr/bin/c++" "-Dnlohmann_json_source=/home/max/Freight/examples/with-external-deps/.deps/json" "/home/max/Freight/examples/with-external-deps/.deps/json/tests/cmake_fetch_content/project")
set_tests_properties(cmake_fetch_content_configure PROPERTIES  FIXTURES_SETUP "cmake_fetch_content" LABELS "git_required;not_reproducible" _BACKTRACE_TRIPLES "/home/max/Freight/examples/with-external-deps/.deps/json/tests/cmake_fetch_content/CMakeLists.txt;2;add_test;/home/max/Freight/examples/with-external-deps/.deps/json/tests/cmake_fetch_content/CMakeLists.txt;0;")
add_test(cmake_fetch_content_build "/usr/bin/cmake" "--build" ".")
set_tests_properties(cmake_fetch_content_build PROPERTIES  FIXTURES_REQUIRED "cmake_fetch_content" LABELS "git_required;not_reproducible" _BACKTRACE_TRIPLES "/home/max/Freight/examples/with-external-deps/.deps/json/tests/cmake_fetch_content/CMakeLists.txt;9;add_test;/home/max/Freight/examples/with-external-deps/.deps/json/tests/cmake_fetch_content/CMakeLists.txt;0;")
