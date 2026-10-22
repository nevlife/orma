// A node for rcl_executor.py: a 10 ms timer publishes to a topic the node subscribes to. Built
// with ORMA_RCL_EXECUTOR against the overhaul's rclcpp it spins an RclExecutor, else a
// SingleThreadedExecutor. Prints its pid, runs on "go", stops on the next line.
//
//     rcl_executor_node PREFIX
#include <cstdio>
#include <iostream>
#include <memory>
#include <string>
#include <thread>

#include "rclcpp/rclcpp.hpp"
#include "std_msgs/msg/header.hpp"
#ifdef ORMA_RCL_EXECUTOR
#include "rclcpp/experimental/executors/rcl_executor.hpp"
using Executor = rclcpp::experimental::executors::RclExecutor;
#else
using Executor = rclcpp::executors::SingleThreadedExecutor;
#endif

using namespace std::chrono_literals;

int main(int argc, char ** argv)
{
  const std::string prefix = argc > 1 ? argv[1] : "/orma_rcl_executor";
  std::printf("%d\n", static_cast<int>(getpid()));
  std::fflush(stdout);
  std::string line;
  std::getline(std::cin, line);

  rclcpp::init(argc, argv);
  auto node = std::make_shared<rclcpp::Node>("orma_rcl_executor");
  auto pub = node->create_publisher<std_msgs::msg::Header>(prefix + "/out", 10);
  int received = 0;
  auto sub = node->create_subscription<std_msgs::msg::Header>(
    prefix + "/out", 10, [&received](std_msgs::msg::Header::ConstSharedPtr) { ++received; });
  int fired = 0;
  auto timer = node->create_wall_timer(10ms, [&]() {
    ++fired;
    std_msgs::msg::Header h;
    h.stamp = node->now();
    pub->publish(h);
  });

  Executor executor;
  executor.add_node(node);
  std::thread spin([&executor]() { executor.spin(); });
  std::getline(std::cin, line);
  executor.cancel();
  spin.join();
  std::printf("timer %d received %d\n", fired, received);
  std::fflush(stdout);
  rclcpp::shutdown();
  return 0;
}
