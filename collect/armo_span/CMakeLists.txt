cmake_minimum_required(VERSION 3.16)
project(armo_span C)

# Called inside every marked callback.
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE RelWithDebInfo)
endif()

include(CheckCCompilerFlag)
find_package(ament_cmake REQUIRED)
find_package(ament_cmake_python REQUIRED)

add_library(armo_span SHARED src/span.c)
set_target_properties(armo_span PROPERTIES C_STANDARD 17 C_VISIBILITY_PRESET hidden)
target_include_directories(armo_span PUBLIC
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include> $<INSTALL_INTERFACE:include/armo_span>)
target_compile_options(armo_span PRIVATE -Wall -Wextra -Wstrict-prototypes)
check_c_compiler_flag(-fno-ipa-icf has_no_ipa_icf)
if(has_no_ipa_icf)
  target_compile_options(armo_span PRIVATE -fno-ipa-icf)
endif()

install(TARGETS armo_span EXPORT armo_spanTargets LIBRARY DESTINATION lib)
install(DIRECTORY include/ DESTINATION include/armo_span)
ament_export_targets(armo_spanTargets HAS_LIBRARY_TARGET)
ament_python_install_package(armo_span)

if(BUILD_TESTING)
  find_package(ament_cmake_test REQUIRED)
  add_executable(armo_span_distinct test/distinct.c)
  target_link_libraries(armo_span_distinct PRIVATE armo_span ${CMAKE_DL_LIBS})
  ament_add_test(span_distinct COMMAND $<TARGET_FILE:armo_span_distinct>
    GENERATE_RESULT_FOR_RETURN_CODE_ZERO TIMEOUT 30)
  add_library(armo_span_fake_shim SHARED test/fake_shim.c)
  ament_add_test(span_decorator
    COMMAND ${CMAKE_CURRENT_SOURCE_DIR}/test/decorator.py $<TARGET_FILE:armo_span_fake_shim>
    ENV PYTHONPATH=${CMAKE_CURRENT_SOURCE_DIR}
    APPEND_LIBRARY_DIRS ${CMAKE_CURRENT_BINARY_DIR}
    GENERATE_RESULT_FOR_RETURN_CODE_ZERO TIMEOUT 30)
endif()

ament_package()
