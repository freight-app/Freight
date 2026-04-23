cmake_minimum_required(VERSION 3.16)
project(demo VERSION 0.3.1 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)

include_directories(include)

add_executable(demo src/main.cpp src/util.cpp)

target_link_libraries(demo PRIVATE m pthread)

find_package(OpenSSL REQUIRED)
target_link_libraries(demo PRIVATE OpenSSL::SSL)

add_subdirectory(vendor/zlib)
