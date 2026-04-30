project('demo', 'cpp',
  version: '0.3.1',
  default_options: ['cpp_std=c++20'])

include_directories('include')

ssl = dependency('openssl')

executable('demo',
  'src/main.cpp',
  'src/util.cpp',
  dependencies: [ssl])
