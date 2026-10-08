cmake_minimum_required(VERSION 3.16)
project(armo_lttng C)

find_package(ament_cmake REQUIRED)
find_package(PkgConfig REQUIRED)
pkg_check_modules(LTTNG_UST REQUIRED IMPORTED_TARGET lttng-ust)

add_library(armo_flux2_lttng SHARED src/flux2_lttng.c)
target_include_directories(armo_flux2_lttng PRIVATE src)
target_link_libraries(armo_flux2_lttng PRIVATE PkgConfig::LTTNG_UST ${CMAKE_DL_LIBS})
target_compile_options(armo_flux2_lttng PRIVATE -Wall -Wextra)
install(TARGETS armo_flux2_lttng LIBRARY DESTINATION lib)

if(BUILD_TESTING)
  find_package(ament_cmake_test REQUIRED)
  ament_add_test(lttng_flux2
    COMMAND ${CMAKE_CURRENT_SOURCE_DIR}/test/lttng_flux2.py $<TARGET_FILE:armo_flux2_lttng>
    GENERATE_RESULT_FOR_RETURN_CODE_ZERO
    TIMEOUT 120)
endif()

ament_package()
