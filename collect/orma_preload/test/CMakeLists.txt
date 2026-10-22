find_package(ament_cmake_test REQUIRED)
find_package(orma_span REQUIRED)
find_package(rcl REQUIRED)

set(preload LD_PRELOAD=$<TARGET_FILE:orma_preload>)

add_executable(orma_preload_caller caller.c)
target_include_directories(orma_preload_caller PRIVATE ../include ${generated})
add_dependencies(orma_preload_caller orma_preload_symbols)
target_link_libraries(orma_preload_caller PRIVATE tracetools::tracetools rcl::rcl
  orma_span::orma_span ${CMAKE_DL_LIBS} -pthread)

add_library(flux_stub SHARED flux_stub.c)
target_compile_definitions(flux_stub PRIVATE STUB_CONTRACT)
add_library(flux_stub_none SHARED flux_stub.c)

ament_add_test(preload_caller
  COMMAND $<TARGET_FILE:orma_preload_caller>
  ENV ${preload} ORMA_THREADS=8
  GENERATE_RESULT_FOR_RETURN_CODE_ZERO TIMEOUT 60)
# The same with ASan's run time loaded ahead of the shim (design 4.4).
execute_process(COMMAND ${CMAKE_C_COMPILER} -print-file-name=libasan.so
  OUTPUT_VARIABLE libasan OUTPUT_STRIP_TRAILING_WHITESPACE)
if(EXISTS "${libasan}")
  ament_add_test(preload_caller_asan
    COMMAND $<TARGET_FILE:orma_preload_caller> asan
    ENV LD_PRELOAD=${libasan}:$<TARGET_FILE:orma_preload> ORMA_THREADS=8
    GENERATE_RESULT_FOR_RETURN_CODE_ZERO TIMEOUT 60)
endif()
# libcuda.so.1 by soname, as orma finds a libcuda that cudart opened RTLD_LOCAL.
add_library(cuda_stub SHARED cuda_stub.c)
target_include_directories(cuda_stub PRIVATE ../include)
set_target_properties(cuda_stub PROPERTIES OUTPUT_NAME cuda SOVERSION 1)
target_link_options(cuda_stub PRIVATE -Wl,-Bsymbolic-functions)
add_library(dds_stub SHARED dds_stub.c)
add_library(delivery_stub SHARED delivery_stub.c)
ament_add_test(preload_collect
  COMMAND $<TARGET_FILE:orma_preload_caller> collect $<TARGET_FILE:cuda_stub>
    $<TARGET_FILE:dds_stub> $<TARGET_FILE:delivery_stub>
  ENV ${preload} ORMA_THREADS=8
  GENERATE_RESULT_FOR_RETURN_CODE_ZERO TIMEOUT 60)

foreach(mode global local none)
  if(mode STREQUAL "none")
    set(stub $<TARGET_FILE:flux_stub_none>)
  else()
    set(stub $<TARGET_FILE:flux_stub>)
  endif()
  ament_add_test(preload_flux_${mode}
    COMMAND $<TARGET_FILE:orma_preload_caller> flux-${mode} ${stub}
    ENV ${preload} ORMA_THREADS=8
    GENERATE_RESULT_FOR_RETURN_CODE_ZERO TIMEOUT 60)
endforeach()

find_package(rclcpp REQUIRED)
find_package(std_msgs REQUIRED)
find_package(std_srvs REQUIRED)

add_executable(shm_reader shm_reader.c)
target_include_directories(shm_reader PRIVATE ../include)

add_executable(orma_preload_node node.cpp)
target_link_libraries(orma_preload_node PRIVATE rclcpp::rclcpp ${std_msgs_TARGETS}
  ${std_srvs_TARGETS})

set(nodes ${CMAKE_CURRENT_SOURCE_DIR}/nodes.py $<TARGET_FILE:shm_reader> $<TARGET_FILE:orma_preload>
  ${CMAKE_CURRENT_SOURCE_DIR}/../include/orma_preload/orma_events.h)
ament_add_test(preload_node_cpp COMMAND ${nodes} cpp $<TARGET_FILE:orma_preload_node>
  GENERATE_RESULT_FOR_RETURN_CODE_ZERO TIMEOUT 120)
ament_add_test(preload_node_py COMMAND ${nodes} py ${CMAKE_CURRENT_SOURCE_DIR}/node.py
  GENERATE_RESULT_FOR_RETURN_CODE_ZERO TIMEOUT 120)
ament_add_test(preload_node_py_bare
  COMMAND ${nodes} py ${CMAKE_CURRENT_SOURCE_DIR}/node.py --without-orma-rclpy
  GENERATE_RESULT_FOR_RETURN_CODE_ZERO TIMEOUT 120)

# Runs when a libflux_ffi is in the workspace; it must carry trace contract 2 (WP1).
find_path(flux_h flux.h)
if(flux_h)
  ament_add_test(preload_flux_real COMMAND ${CMAKE_CURRENT_SOURCE_DIR}/flux_real.py
    $<TARGET_FILE:shm_reader> $<TARGET_FILE:orma_preload>
    ${CMAKE_CURRENT_SOURCE_DIR}/../include/orma_preload/orma_events.h
    GENERATE_RESULT_FOR_RETURN_CODE_ZERO TIMEOUT 120)
endif()

add_executable(orma_preload_bench bench.c)
target_include_directories(orma_preload_bench PRIVATE ../include)
target_link_libraries(orma_preload_bench PRIVATE tracetools::tracetools -pthread)
