// Publishers and subscriptions of the delivery path tests (docs/instrumentation.md 2.13), in
// one process so that the broker can route between them.
//   delivery_node NAME [pub=TOPIC:TYPE:BYTES:HZ[:loan]]... [sub=TOPIC:TYPE]... [seconds=S]
// TYPE: image (sensor_msgs/Image, BYTES of data), plain (test_rmw_zenoh_cpp/PlainPayload,
// 4 MiB), leaf (test_rmw_zenoh_cpp/PlainLeaf); DELIVERY_STOCK builds image only. Prints "ready",
// runs S seconds after a line on stdin, prints "pub TOPIC N", "sub TOPIC N", "cpu_us N" (the
// process's CPU time over the run) and "done", and exits after another line.
#include <sys/resource.h>

#include <chrono>
#include <cstdint>
#include <iostream>
#include <map>
#include <memory>
#include <string>
#include <thread>
#include <vector>

#include "rclcpp/rclcpp.hpp"
#include "sensor_msgs/msg/image.hpp"
#ifndef DELIVERY_STOCK
#include "test_rmw_zenoh_cpp/msg/plain_leaf.hpp"
#include "test_rmw_zenoh_cpp/msg/plain_payload.hpp"
#endif

using Image = sensor_msgs::msg::Image;
#ifndef DELIVERY_STOCK
using Plain = test_rmw_zenoh_cpp::msg::PlainPayload;
using Leaf = test_rmw_zenoh_cpp::msg::PlainLeaf;
#else
using Plain = Image;
using Leaf = Image;
#endif

static std::vector<std::string> split(const std::string & s)
{
  std::vector<std::string> out;
  size_t at = 0;
  for (size_t c; (c = s.find(':', at)) != std::string::npos; at = c + 1) {
    out.push_back(s.substr(at, c - at));
  }
  out.push_back(s.substr(at));
  return out;
}

static int64_t cpu_us()
{
  rusage u{};
  getrusage(RUSAGE_SELF, &u);
  return (u.ru_utime.tv_sec + u.ru_stime.tv_sec) * 1000000LL + u.ru_utime.tv_usec +
         u.ru_stime.tv_usec;
}

static int64_t now_ns()
{
  return std::chrono::duration_cast<std::chrono::nanoseconds>(
    std::chrono::system_clock::now().time_since_epoch()).count();
}

template<typename M>
static void stamp(M & m, int64_t ns)
{
  if constexpr (std::is_same_v<M, Image>) {
    m.header.stamp.sec = static_cast<int32_t>(ns / 1000000000);
    m.header.stamp.nanosec = static_cast<uint32_t>(ns % 1000000000);
#ifndef DELIVERY_STOCK
  } else if constexpr (std::is_same_v<M, Plain>) {
    m.stamp = ns;
  } else {
    m.value = static_cast<uint32_t>(ns);
#endif
  }
}

struct Pub
{
  std::string topic;
  std::function<void()> once;
  uint64_t n = 0;
};

template<typename M>
static void add_pub(rclcpp::Node & node, Pub & p, size_t bytes, bool loan)
{
  auto pub = node.create_publisher<M>(p.topic, rclcpp::QoS(10).reliable());
  auto msg = std::make_shared<M>();
  if constexpr (std::is_same_v<M, Image>) {
    msg->height = 1;
    msg->width = static_cast<uint32_t>(bytes);
    msg->step = msg->width;
    msg->encoding = "mono8";
    msg->data.resize(bytes);
  }
  p.once = [pub, msg, loan, &p]() {
      if (loan) {
        auto l = pub->borrow_loaned_message();
        stamp(l.get(), now_ns());
        pub->publish(std::move(l));
      } else {
        stamp(*msg, now_ns());
        pub->publish(*msg);
      }
      ++p.n;
    };
}

int main(int argc, char ** argv)
{
  rclcpp::init(1, argv);
  auto node = std::make_shared<rclcpp::Node>(argc > 1 ? argv[1] : "delivery_node");
  std::vector<std::unique_ptr<Pub>> pubs;
  std::vector<double> rates;
  std::map<std::string, uint64_t> got;
  std::vector<rclcpp::SubscriptionBase::SharedPtr> subs;
  double seconds = 3.0;
  for (int i = 2; i < argc; ++i) {
    std::string a = argv[i];
    auto eq = a.find('=');
    auto key = a.substr(0, eq);
    auto f = split(a.substr(eq + 1));
    if (key == "seconds") {
      seconds = std::stod(f[0]);
    } else if (key == "pub") {
      auto p = std::make_unique<Pub>();
      p->topic = f[0];
      size_t bytes = std::stoul(f[2]);
      bool loan = f.size() > 4 && f[4] == "loan";
      if (f[1] == "image") {
        add_pub<Image>(*node, *p, bytes, loan);
      } else if (f[1] == "plain") {
        add_pub<Plain>(*node, *p, bytes, loan);
      } else {
        add_pub<Leaf>(*node, *p, bytes, loan);
      }
      rates.push_back(std::stod(f[3]));
      pubs.push_back(std::move(p));
    } else if (key == "sub") {
      auto topic = f[0];
      got[topic] = 0;
      auto * count = &got[topic];
      auto qos = rclcpp::QoS(10).reliable();
      if (f[1] == "image") {
        subs.push_back(node->create_subscription<Image>(
          topic, qos, [count](std::shared_ptr<const Image>) {++*count;}));
      } else if (f[1] == "plain") {
        subs.push_back(node->create_subscription<Plain>(
          topic, qos, [count](std::shared_ptr<const Plain>) {++*count;}));
      } else {
        subs.push_back(node->create_subscription<Leaf>(
          topic, qos, [count](std::shared_ptr<const Leaf>) {++*count;}));
      }
    }
  }
  std::vector<rclcpp::TimerBase::SharedPtr> timers;
  for (size_t i = 0; i < pubs.size(); ++i) {
    auto period = std::chrono::nanoseconds(static_cast<int64_t>(1e9 / rates[i]));
    auto * p = pubs[i].get();
    timers.push_back(node->create_wall_timer(period, [p]() {p->once();}));
    timers.back()->cancel();
  }
  rclcpp::executors::SingleThreadedExecutor exec;
  exec.add_node(node);
  std::thread spin([&exec]() {exec.spin();});
  std::cout << "ready" << std::endl;
  std::string line;
  std::getline(std::cin, line);
  int64_t cpu0 = cpu_us();
  for (auto & t : timers) {
    t->reset();
  }
  std::this_thread::sleep_for(std::chrono::duration<double>(seconds));
  for (auto & t : timers) {
    t->cancel();
  }
  std::this_thread::sleep_for(std::chrono::milliseconds(300));
  int64_t cpu = cpu_us() - cpu0;
  for (auto & p : pubs) {
    std::cout << "pub " << p->topic << " " << p->n << std::endl;
  }
  for (auto & [topic, n] : got) {
    std::cout << "sub " << topic << " " << n << std::endl;
  }
  std::cout << "cpu_us " << cpu << std::endl;
  std::cout << "done" << std::endl;
  std::getline(std::cin, line);
  exec.cancel();
  spin.join();
  rclcpp::shutdown();
  return 0;
}
