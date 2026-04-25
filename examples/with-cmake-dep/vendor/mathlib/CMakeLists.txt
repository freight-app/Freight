cmake_minimum_required(VERSION 3.10)
project(mathlib VERSION 1.0.0 LANGUAGES CXX)

add_library(mathlib STATIC src/mathlib.cpp)
target_include_directories(mathlib PUBLIC include)
