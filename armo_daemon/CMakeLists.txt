cmake_minimum_required(VERSION 3.16)
project(armo_daemon NONE)

# armod is the cargo workspace one level up. Its build script builds the eBPF crate with the
# pinned nightly (build.rs), which needs rustup's cargo and bpf-linker on PATH.

find_package(ament_cmake REQUIRED)

set(cargo_home $ENV{CARGO_HOME})
if(NOT cargo_home)
  set(cargo_home $ENV{HOME}/.cargo)
endif()
find_program(CARGO cargo PATHS ${cargo_home}/bin NO_DEFAULT_PATH)
if(NOT CARGO)
  message(FATAL_ERROR "rustup's cargo not found under ${cargo_home}/bin: install rustup")
endif()
find_program(BPF_LINKER bpf-linker)
if(NOT BPF_LINKER)
  message(FATAL_ERROR "bpf-linker not on PATH: see README.md, Building")
endif()

set(WORKSPACE ${CMAKE_CURRENT_SOURCE_DIR}/..)
set(CARGO_DIR ${CMAKE_CURRENT_BINARY_DIR}/cargo)
set(CARGO_ENV CARGO_TARGET_DIR=${CARGO_DIR} PATH=${cargo_home}/bin:$ENV{PATH})

# Always runs; cargo decides what is out of date. MAKEFLAGS and MFLAGS name make's jobserver
# pipes, which a custom target's command does not inherit.
add_custom_target(armod ALL
  COMMAND ${CMAKE_COMMAND} -E env --unset=MAKEFLAGS --unset=MFLAGS ${CARGO_ENV}
    ${CARGO} build --release --locked -p armod
  WORKING_DIRECTORY ${WORKSPACE}
  USES_TERMINAL
  COMMENT "cargo build armod")
install(PROGRAMS ${CARGO_DIR}/release/armod DESTINATION bin)

if(BUILD_TESTING)
  find_package(ament_cmake_test REQUIRED)
  ament_add_test(armod_cargo_test
    COMMAND ${CARGO} test --release --locked
    GENERATE_RESULT_FOR_RETURN_CODE_ZERO
    WORKING_DIRECTORY ${WORKSPACE}
    TIMEOUT 600
    ENV ${CARGO_ENV})
endif()

ament_package()
