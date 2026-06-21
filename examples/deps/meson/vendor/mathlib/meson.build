project('mathlib', 'cpp', default_options: ['cpp_std=c++17'])

inc = include_directories('include')
mathlib = static_library(
  'mathlib',
  'src/mathlib.cpp',
  include_directories: inc,
  install: true,
)
install_headers('include/mathlib.h')
