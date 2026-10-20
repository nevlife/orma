cmake_minimum_required(VERSION 3.16)
project(orma_integration CXX)

if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()
set(CMAKE_CXX_STANDARD 17)

find_package(ament_cmake REQUIRED)
find_package(rclcpp REQUIRED)
find_package(rclcpp_components REQUIRED)
find_package(std_msgs REQUIRED)

add_library(orma_integration_nodes SHARED src/nodes.cpp)
target_link_libraries(orma_integration_nodes
  rclcpp::rclcpp rclcpp_components::component ${std_msgs_TARGETS})

rclcpp_components_register_node(orma_integration_nodes
  PLUGIN "orma_integration::Pub" EXECUTABLE tc_pub)
rclcpp_components_register_node(orma_integration_nodes
  PLUGIN "orma_integration::Relay" EXECUTABLE tc_relay)
rclcpp_components_register_node(orma_integration_nodes
  PLUGIN "orma_integration::Sink" EXECUTABLE tc_sink)
rclcpp_components_register_node(orma_integration_nodes
  PLUGIN "orma_integration::Burner" EXECUTABLE burner)

install(TARGETS orma_integration_nodes LIBRARY DESTINATION lib RUNTIME DESTINATION bin)
install(PROGRAMS scripts/tc_nodes.py DESTINATION lib/${PROJECT_NAME})

ament_package()
