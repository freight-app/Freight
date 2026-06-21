cmake_minimum_required(VERSION 3.10)
project(mathlib VERSION 1.0.0 LANGUAGES CXX)

add_library(mathlib STATIC src/mathlib.cpp)
target_include_directories(mathlib PUBLIC include)

# Install rules so the freight `cmake` plugin can stage headers + the library.
install(TARGETS mathlib ARCHIVE DESTINATION lib)
install(DIRECTORY include/ DESTINATION include)
