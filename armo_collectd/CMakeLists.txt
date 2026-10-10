cmake_minimum_required(VERSION 3.16)
project(armo_collectd NONE)

# armo-collectd and armo-analyze come from the cargo workspace one level up.

find_package(ament_cmake REQUIRED)

set(cargo_home $ENV{CARGO_HOME})
if(NOT cargo_home)
  set(cargo_home $ENV{HOME}/.cargo)
endif()
find_program(CARGO cargo PATHS ${cargo_home}/bin NO_DEFAULT_PATH)
if(NOT CARGO)
  message(FATAL_ERROR "rustup's cargo not found under ${cargo_home}/bin: install rustup")
endif()

set(WORKSPACE ${CMAKE_CURRENT_SOURCE_DIR}/..)
set(CARGO_DIR ${CMAKE_CURRENT_BINARY_DIR}/cargo)
set(CARGO_ENV CARGO_TARGET_DIR=${CARGO_DIR} PATH=${cargo_home}/bin:$ENV{PATH})

# Always runs; cargo decides what is out of date. MAKEFLAGS and MFLAGS name make's jobserver
# pipes, which a custom target's command does not inherit.
add_custom_target(armo_collectd_bins ALL
  COMMAND ${CMAKE_COMMAND} -E env --unset=MAKEFLAGS --unset=MFLAGS ${CARGO_ENV}
    ${CARGO} build --release --locked -p armo-collectd
  WORKING_DIRECTORY ${WORKSPACE}
  USES_TERMINAL
  COMMENT "cargo build armo-collectd")
install(PROGRAMS ${CARGO_DIR}/release/armo-collectd ${CARGO_DIR}/release/armo-analyze
  DESTINATION bin)

# The systemd user unit (design 5.3). Enable with
# systemctl --user link <prefix>/share/armo_collectd/systemd/armo-collectd.service
configure_file(systemd/armo-collectd.service.in ${CMAKE_CURRENT_BINARY_DIR}/armo-collectd.service
  @ONLY)
install(FILES ${CMAKE_CURRENT_BINARY_DIR}/armo-collectd.service
  DESTINATION share/${PROJECT_NAME}/systemd)

if(BUILD_TESTING)
  find_package(ament_cmake_test REQUIRED)
  ament_add_test(armo_collectd_cargo_test
    COMMAND ${CARGO} test --release --locked -p armo-collectd
    GENERATE_RESULT_FOR_RETURN_CODE_ZERO
    WORKING_DIRECTORY ${WORKSPACE}
    TIMEOUT 600
    ENV ${CARGO_ENV})
endif()

ament_package()
