// A C++ node with known callback counts for nodes.py. After rclcpp::init it waits for a line on
// stdin, so that the reader has armed the file before the entities exist. Then: timer t1 (2 ms)
// fires T1 times; each publishes a Header and spins a second executor until the subscription has
// it, so the subscription callback runs nested inside t1's. Timer t2 (3 ms) fires T2 times on its
// own thread, and the service answers SRV calls. Prints the counts as JSON.
#include "rclcpp/rclcpp.hpp"

#include "std_msgs/msg/header.hpp"
#include "std_srvs/srv/trigger.hpp"

#include <chrono>
#include <cstdio>
#include <iostream>
#include <memory>
#include <string>
#include <thread>

using namespace std::chrono_literals;
using std_msgs::msg::Header;
using std_srvs::srv::Trigger;

constexpr int T1 = 100;
constexpr int T2 = 50;
constexpr int SRV = 20;

int main(int argc, char ** argv)
{
  rclcpp::init(argc, argv);
  std::string go;
  std::getline(std::cin, go);

  auto a = std::make_shared<rclcpp::Node>("armo_test_a");
  auto b = std::make_shared<rclcpp::Node>("armo_test_b");
  auto c = std::make_shared<rclcpp::Node>("armo_test_c");
  rclcpp::executors::SingleThreadedExecutor ea, eb, ec;
  ea.add_node(a);
  eb.add_node(b);
  ec.add_node(c);

  int t1 = 0, sub = 0, t2 = 0, srv = 0, answered = 0;
  auto pub = a->create_publisher<Header>("/armo_test/chatter", 10);
  auto subscription =
    b->create_subscription<Header>("/armo_test/chatter", 10, [&sub](const Header &) { sub++; });
  rclcpp::TimerBase::SharedPtr timer1;
  timer1 = a->create_wall_timer(2ms, [&]() {
    Header h;
    h.stamp.sec = 1000 + t1;
    h.stamp.nanosec = 7;
    t1++;
    pub->publish(h);
    while (sub < t1) eb.spin_once(100ms);
    if (t1 == T1) timer1->cancel();
  });
  auto service = a->create_service<Trigger>(
    "/armo_test/trigger", [&srv](Trigger::Request::SharedPtr, Trigger::Response::SharedPtr res) {
      srv++;
      res->success = true;
    });
  auto client = b->create_client<Trigger>("/armo_test/trigger");

  rclcpp::TimerBase::SharedPtr timer2;
  timer2 = c->create_wall_timer(3ms, [&]() {
    if (++t2 == T2) timer2->cancel();
  });
  std::thread other([&]() {
    while (t2 < T2) ec.spin_once(100ms);
  });

  while (t1 < T1) ea.spin_once(100ms);
  while (!client->wait_for_service(100ms)) {
  }
  for (int i = 0; i < SRV; i++) {
    client->async_send_request(
      std::make_shared<Trigger::Request>(),
      [&answered](rclcpp::Client<Trigger>::SharedFuture) { answered++; });
    while (answered <= i) {
      ea.spin_once(10ms);
      eb.spin_once(10ms);
    }
  }
  other.join();
  std::printf("{\"t1\": %d, \"sub\": %d, \"t2\": %d, \"srv\": %d}\n", t1, sub, t2, srv);
  rclcpp::shutdown();
  return 0;
}
