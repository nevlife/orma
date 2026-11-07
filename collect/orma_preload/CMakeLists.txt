cmake_minimum_required(VERSION 3.16)
project(orma_preload C CXX)

# The shim runs inside every observed call: an unset build type would leave it unoptimized.
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE RelWithDebInfo)
endif()

find_package(ament_cmake REQUIRED)
find_package(Python3 REQUIRED COMPONENTS Interpreter)
# For headers, library paths and versions only: the shim links no ROS library, so loading it into
# a process that is not a ROS node pulls in nothing.
find_package(tracetools REQUIRED)
find_package(rclcpp REQUIRED)
find_package(rmw REQUIRED)

get_target_property(rmw_include rmw::rmw INTERFACE_INCLUDE_DIRECTORIES)
find_file(rmw_types_h rmw/types.h PATHS ${rmw_include} NO_DEFAULT_PATH REQUIRED)
file(STRINGS ${rmw_types_h} gid_line REGEX "#define RMW_GID_STORAGE_SIZE")
string(REGEX MATCH "[0-9]+" rmw_gid_size "${gid_line}")

set(generated ${CMAKE_CURRENT_BINARY_DIR}/generated)
add_custom_command(
  OUTPUT ${generated}/orma_preload/orma_tracepoints.h ${generated}/orma_preload/orma_rclcpp_symbols.h
  COMMAND Python3::Interpreter ${CMAKE_CURRENT_SOURCE_DIR}/cmake/gen_symbols.py ${CMAKE_NM}
    $<TARGET_FILE:tracetools::tracetools> $<TARGET_FILE:rclcpp::rclcpp> ${rclcpp_VERSION_MAJOR}
    ${generated}
  DEPENDS cmake/gen_symbols.py $<TARGET_FILE:tracetools::tracetools> $<TARGET_FILE:rclcpp::rclcpp>
  VERBATIM)
add_custom_target(orma_preload_symbols DEPENDS
  ${generated}/orma_preload/orma_tracepoints.h ${generated}/orma_preload/orma_rclcpp_symbols.h)

# The shim runs on threads whose stack size it does not know (DDS, SDK): frames stay small.
set(warnings -Wall -Wextra -Werror -Wshadow -Wstrict-prototypes -Wframe-larger-than=2048
  -Wstack-usage=2048)
# Every caught call passes through these functions; the canary in each costs 3 to 5 ns per
# callback pair (design 8.1), and none of them copies into a stack buffer.
set(shim_flags ${warnings} -fno-stack-protector)

# The interception code both backends link whole (design 4.4): liborma_preload.so here,
# liborma_lttng.so in orma_lttng.
add_library(orma_intercept STATIC
  src/intercept_cuda.c src/intercept_dds.c src/intercept_flux.c src/intercept_malloc.c
  src/intercept_delivery.c src/intercept_rcl.c src/intercept_rclcpp.c src/intercept_ros.c
  src/intercept_span.c
  src/resolve.c)
add_dependencies(orma_intercept orma_preload_symbols)
set_target_properties(orma_intercept PROPERTIES
  POSITION_INDEPENDENT_CODE ON C_STANDARD 17 C_EXTENSIONS ON C_VISIBILITY_PRESET hidden)
target_include_directories(orma_intercept PUBLIC
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include> $<BUILD_INTERFACE:${generated}>
  $<INSTALL_INTERFACE:include/orma_preload>)
target_include_directories(orma_intercept SYSTEM PRIVATE
  $<TARGET_PROPERTY:tracetools::tracetools,INTERFACE_INCLUDE_DIRECTORIES>)
target_compile_definitions(orma_intercept PRIVATE
  ORMA_TRACETOOLS_MAJOR=${tracetools_VERSION_MAJOR} ORMA_RMW_GID_SIZE=${rmw_gid_size})
target_compile_options(orma_intercept PRIVATE ${shim_flags})

# The shared memory file and thread slots both backends keep (design 13.1, 15.2).
add_library(orma_slots STATIC src/slots.c)
set_target_properties(orma_slots PROPERTIES
  POSITION_INDEPENDENT_CODE ON C_STANDARD 17 C_EXTENSIONS ON C_VISIBILITY_PRESET hidden)
target_include_directories(orma_slots PUBLIC
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include> $<INSTALL_INTERFACE:include/orma_preload>)
target_compile_options(orma_slots PRIVATE ${shim_flags})

add_library(orma_preload SHARED src/shm_backend.c)
set_target_properties(orma_preload PROPERTIES
  C_STANDARD 17 C_EXTENSIONS ON C_VISIBILITY_PRESET hidden)
target_include_directories(orma_preload PRIVATE include)
target_compile_options(orma_preload PRIVATE ${shim_flags})
target_link_libraries(orma_preload PRIVATE
  -Wl,--whole-archive orma_intercept -Wl,--no-whole-archive orma_slots ${CMAKE_DL_LIBS} -pthread
  -Wl,-z,defs -Wl,-z,now)

install(TARGETS orma_preload LIBRARY DESTINATION lib)
install(TARGETS orma_intercept orma_slots EXPORT orma_interceptTargets ARCHIVE DESTINATION lib)
install(DIRECTORY include/ ${generated}/ DESTINATION include/orma_preload)
ament_export_targets(orma_interceptTargets)

if(BUILD_TESTING)
  add_subdirectory(test)
endif()

ament_package()
