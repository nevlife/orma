cmake_minimum_required(VERSION 3.16)
project(armo_integration CXX)

if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()
set(CMAKE_CXX_STANDARD 17)

find_package(ament_cmake REQUIRED)
find_package(rclcpp REQUIRED)
find_package(rclcpp_components REQUIRED)
find_package(std_msgs REQUIRED)

add_library(armo_integration_nodes SHARED src/nodes.cpp)
target_link_libraries(armo_integration_nodes
  rclcpp::rclcpp rclcpp_components::component ${std_msgs_TARGETS})

rclcpp_components_register_node(armo_integration_nodes
  PLUGIN "armo_integration::Pub" EXECUTABLE tc_pub)
rclcpp_components_register_node(armo_integration_nodes
  PLUGIN "armo_integration::Relay" EXECUTABLE tc_relay)
rclcpp_components_register_node(armo_integration_nodes
  PLUGIN "armo_integration::Sink" EXECUTABLE tc_sink)
rclcpp_components_register_node(armo_integration_nodes
  PLUGIN "armo_integration::Burner" EXECUTABLE burner)

install(TARGETS armo_integration_nodes LIBRARY DESTINATION lib RUNTIME DESTINATION bin)
install(PROGRAMS scripts/tc_nodes.py DESTINATION lib/${PROJECT_NAME})

ament_package()
