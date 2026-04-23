cmake_minimum_required(VERSION 3.16)
project(migrated-from-cmake VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)

include_directories(include)

add_executable(migrated-from-cmake src/main.cpp src/greeter.cpp)

target_link_libraries(migrated-from-cmake PRIVATE pthread)
