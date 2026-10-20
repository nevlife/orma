// A workload for agreement.py: `lttng_node pub COUNT` waits DELAY, then publishes COUNT
// std_msgs/Header at 200 Hz on /orma_lttng; `lttng_node sub` takes them. Both spin until SIGINT.
#include <rclcpp/rclcpp.hpp>

#include <std_msgs/msg/header.hpp>

#include <chrono>
#include <memory>
#include <string>

using namespace std::chrono_literals;

int main(int argc, char ** argv)
{
  rclcpp::init(argc, argv);
  const std::string role = argc > 1 ? argv[1] : "sub";
  auto node = std::make_shared<rclcpp::Node>("orma_lttng_" + role);
  rclcpp::Publisher<std_msgs::msg::Header>::SharedPtr pub;
  rclcpp::Subscription<std_msgs::msg::Header>::SharedPtr sub;
  rclcpp::TimerBase::SharedPtr timer;
  int left = argc > 2 ? std::stoi(argv[2]) : 0;
  const auto start = node->now() + rclcpp::Duration(3s);
  if (role == "pub") {
    pub = node->create_publisher<std_msgs::msg::Header>("/orma_lttng", 10);
    timer = node->create_wall_timer(5ms, [&]() {
      if (left == 0 || node->now() < start) return;
      std_msgs::msg::Header h;
      h.stamp = node->now();
      pub->publish(h);
      left--;
    });
  } else {
    sub = node->create_subscription<std_msgs::msg::Header>(
      "/orma_lttng", 10, [](std_msgs::msg::Header::ConstSharedPtr) {});
  }
  rclcpp::spin(node);
  rclcpp::shutdown();
  return 0;
}
