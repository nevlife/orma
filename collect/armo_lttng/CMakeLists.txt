cmake_minimum_required(VERSION 3.16)
project(armo_lttng C CXX)

# Every caught call runs through this library, as through libarmo_preload.so.
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE RelWithDebInfo)
endif()

find_package(ament_cmake REQUIRED)
find_package(armo_preload REQUIRED)
find_package(tracetools REQUIRED)
find_package(PkgConfig REQUIRED)
pkg_check_modules(LTTNG_UST REQUIRED IMPORTED_TARGET lttng-ust)

# A libtracetools without LTTng (Humble) emits no ros2 events: this library does, in Humble's
# tracetools 4 layout (ros2_tp.h).
get_target_property(tracetools_include tracetools::tracetools INTERFACE_INCLUDE_DIRECTORIES)
find_file(tracetools_config tracetools/config.h PATHS ${tracetools_include} NO_DEFAULT_PATH
  REQUIRED)
file(STRINGS ${tracetools_config} lttng_enabled REGEX "^#define TRACETOOLS_LTTNG_ENABLED")
if(lttng_enabled)
  set(ros2_provider 0)
elseif(tracetools_VERSION_MAJOR LESS 8)
  set(ros2_provider 1)
else()
  set(ros2_provider 0)
  message(WARNING "libtracetools ${tracetools_VERSION} has no LTTng and no ros2 layout here: "
    "LTTng sessions get armo and flux2 events but no ros2 events")
endif()

set(sources src/lttng_backend.c src/tp_armo.c src/tp_flux2.c)
if(ros2_provider)
  list(APPEND sources src/tp_ros2.c)
endif()

add_library(armo_lttng SHARED ${sources})
set_target_properties(armo_lttng PROPERTIES
  C_STANDARD 17 C_EXTENSIONS ON C_VISIBILITY_PRESET hidden)
target_include_directories(armo_lttng PRIVATE include)
target_compile_definitions(armo_lttng PRIVATE
  ARMO_ROS2_PROVIDER=${ros2_provider} ARMO_TRACETOOLS_VERSION="${tracetools_VERSION}")
target_compile_options(armo_lttng PRIVATE -Wall -Wextra -Wshadow -Wstrict-prototypes)
target_link_libraries(armo_lttng PRIVATE
  -Wl,--whole-archive armo_preload::armo_intercept -Wl,--no-whole-archive armo_preload::armo_slots
  PkgConfig::LTTNG_UST ${CMAKE_DL_LIBS} -pthread -Wl,-z,defs -Wl,-z,now)
install(TARGETS armo_lttng LIBRARY DESTINATION lib)

if(BUILD_TESTING)
  find_package(ament_cmake_test REQUIRED)
  find_package(flux2_ffi QUIET)

  add_executable(lttng_caller test/caller.c)
  # armo_tracepoints.h only: linking the interception code would define ros_trace_* here.
  target_include_directories(lttng_caller PRIVATE
    $<TARGET_PROPERTY:armo_preload::armo_intercept,INTERFACE_INCLUDE_DIRECTORIES>)
  find_package(armo_span REQUIRED)
  target_link_libraries(lttng_caller PRIVATE tracetools::tracetools armo_span::armo_span
    ${CMAKE_DL_LIBS})
  # armo_preload's stand-ins for libcuda.so.1 and the DDS libraries.
  set(stubs ${CMAKE_CURRENT_SOURCE_DIR}/../armo_preload/test)
  add_library(lttng_cuda_stub SHARED ${stubs}/cuda_stub.c)
  target_include_directories(lttng_cuda_stub PRIVATE
    $<TARGET_PROPERTY:armo_preload::armo_intercept,INTERFACE_INCLUDE_DIRECTORIES>)
  set_target_properties(lttng_cuda_stub PROPERTIES OUTPUT_NAME cuda SOVERSION 1)
  target_link_options(lttng_cuda_stub PRIVATE -Wl,-Bsymbolic-functions)
  add_library(lttng_dds_stub SHARED ${stubs}/dds_stub.c)
  target_compile_definitions(lttng_caller PRIVATE
    ARMO_TRACETOOLS_MAJOR=${tracetools_VERSION_MAJOR} ARMO_FLUX2=$<BOOL:${flux2_ffi_FOUND}>)
  if(flux2_ffi_FOUND)
    target_link_libraries(lttng_caller PRIVATE flux2::ffi)
  endif()
  # The workload of agreement.py and humble_agreement.py.
  find_package(rclcpp REQUIRED)
  find_package(std_msgs REQUIRED)
  add_executable(lttng_node test/node.cpp)
  target_link_libraries(lttng_node PRIVATE rclcpp::rclcpp ${std_msgs_TARGETS})

  ament_add_test(lttng_events
    COMMAND ${CMAKE_CURRENT_SOURCE_DIR}/test/lttng_events.py $<TARGET_FILE:armo_lttng>
      $<TARGET_FILE:lttng_caller> $<TARGET_FILE:lttng_cuda_stub> $<TARGET_FILE:lttng_dds_stub>
    GENERATE_RESULT_FOR_RETURN_CODE_ZERO
    TIMEOUT 120)
  ament_add_test(lttng_lifecycle
    COMMAND ${CMAKE_CURRENT_SOURCE_DIR}/test/lifecycle.py $<TARGET_FILE:armo_lttng>
      $<TARGET_FILE:lttng_caller>
    GENERATE_RESULT_FOR_RETURN_CODE_ZERO
    TIMEOUT 300)

  # The real flux2: a workspace with flux2_ffi (trace contract 2).
  if(flux2_ffi_FOUND)
    ament_add_test(lttng_flux2
      COMMAND ${CMAKE_CURRENT_SOURCE_DIR}/test/lttng_flux2.py $<TARGET_FILE:armo_lttng>
      GENERATE_RESULT_FOR_RETURN_CODE_ZERO
      TIMEOUT 120)

    # The installed armo-collectd, libarmo_preload.so and libarmo_lttng.so with real nodes.
    ament_add_test(lttng_agreement
      COMMAND ${CMAKE_CURRENT_SOURCE_DIR}/test/agreement.py $<TARGET_FILE:lttng_node>
      GENERATE_RESULT_FOR_RETURN_CODE_ZERO
      TIMEOUT 180)
  endif()
endif()

ament_package()
