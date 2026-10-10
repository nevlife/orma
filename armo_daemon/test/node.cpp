// A ROS node that runs known numbers of callbacks, for accuracy.py to count under armod. It
// prints its pid and waits for "go" so armod can attach before any entity exists. Then a timer
// publishes a Header to a topic the node also subscribes to and calls a service of its own; a
// second callback group runs a second timer on its own executor thread. Builds with rclcpp 16
// (Humble) and 28 (Jazzy).
//
//     node PREFIX TICKS
#include <atomic>
#include <chrono>
#include <cstdio>
#include <iostream>
#include <memory>
#include <string>
#include <thread>

#include "rclcpp/rclcpp.hpp"
#include "std_msgs/msg/header.hpp"
#include "std_srvs/srv/trigger.hpp"

using namespace std::chrono_literals;

int main(int argc, char ** argv)
{
  const std::string prefix = argc > 1 ? argv[1] : "/armo_acc";
  const int ticks = argc > 2 ? std::stoi(argv[2]) : 50;
  const int calls = ticks / 2;
  const int ticks2 = ticks / 2;
  std::printf("%d\n", static_cast<int>(getpid()));
  std::fflush(stdout);
  std::string line;
  std::getline(std::cin, line);

  rclcpp::init(argc, argv);
  auto node = std::make_shared<rclcpp::Node>("armo_acc");
  rclcpp::QoS qos = rclcpp::QoS(rclcpp::KeepLast(1000)).reliable();
  auto pub = node->create_publisher<std_msgs::msg::Header>(prefix + "/out", qos);
  int received = 0;
  auto sub = node->create_subscription<std_msgs::msg::Header>(
    prefix + "/out", qos, [&received](std_msgs::msg::Header::ConstSharedPtr) { ++received; });
  int served = 0;
  auto srv = node->create_service<std_srvs::srv::Trigger>(
    prefix + "/srv", [&served](
                       const std::shared_ptr<std_srvs::srv::Trigger::Request>,
                       std::shared_ptr<std_srvs::srv::Trigger::Response> res) {
      ++served;
      res->success = true;
    });
  auto client = node->create_client<std_srvs::srv::Trigger>(prefix + "/srv");
  client->wait_for_service(5s);

  // Every invocation counts, also the ones past the target: armod sees each.
  int fired = 0;
  int timer_runs = 0;
  auto timer = node->create_wall_timer(10ms, [&]() {
    ++timer_runs;
    if (fired >= ticks) return;
    ++fired;
    std_msgs::msg::Header h;
    h.stamp = node->now();
    h.frame_id = "f";
    pub->publish(h);
    if (fired <= calls) client->async_send_request(std::make_shared<std_srvs::srv::Trigger::Request>());
  });

  auto group = node->create_callback_group(rclcpp::CallbackGroupType::MutuallyExclusive, false);
  std::atomic<int> fired2{0};
  auto timer2 = node->create_wall_timer(15ms, [&fired2]() { ++fired2; }, group);
  rclcpp::executors::SingleThreadedExecutor second;
  second.add_callback_group(group, node->get_node_base_interface());
  std::thread other([&second]() { second.spin(); });

  rclcpp::executors::SingleThreadedExecutor exec;
  exec.add_node(node);
  auto deadline = std::chrono::steady_clock::now() + 20s;
  while (rclcpp::ok() && std::chrono::steady_clock::now() < deadline &&
         (fired < ticks || received < ticks || served < calls || fired2 < ticks2)) {
    exec.spin_once(20ms);
  }
  timer->cancel();
  timer2->cancel();
  second.cancel();
  other.join();
  std::printf(
    "timer %d timer2 %d received %d served %d published %d\n", timer_runs, fired2.load(), received,
    served, fired);
  std::fflush(stdout);
  rclcpp::shutdown();
  std::getline(std::cin, line);
  return 0;
}
