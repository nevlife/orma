// Test nodes of orma_integration. Pub, Relay and Sink carry `topics` independent topics; Pub writes
// {CLOCK_MONOTONIC ns, seq} into the first 16 payload bytes right before publish and Sink keeps the
// arrival time at callback entry, written to <tag>_t<i>.csv in the working directory at the end.
// Burner spends `busy_ms` of its thread's CPU time, then sleeps `sleep_ms`, every `period_ms`.
// rclcpp 16 (Humble) builds this file unchanged (design 17).
#include <rclcpp/rclcpp.hpp>
#include <rclcpp_components/register_node_macro.hpp>

#include <std_msgs/msg/u_int8_multi_array.hpp>

#include <time.h>

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <fstream>
#include <memory>
#include <string>
#include <thread>
#include <vector>

namespace orma_integration
{

using Msg = std_msgs::msg::UInt8MultiArray;

uint64_t now_ns(clockid_t clock)
{
  timespec ts{};
  clock_gettime(clock, &ts);
  return static_cast<uint64_t>(ts.tv_sec) * 1000000000ull + static_cast<uint64_t>(ts.tv_nsec);
}

std::string topic(const std::string & prefix, int i)
{
  return prefix + std::to_string(i);
}

class Pub : public rclcpp::Node
{
public:
  explicit Pub(const rclcpp::NodeOptions & o) : Node("tc_pub", o)
  {
    const int n = declare_parameter("topics", 4);
    const double hz = declare_parameter("rate_hz", 1000.0);
    const auto out = declare_parameter("out", std::string("/tc/a"));
    // The stamp takes the first 16 bytes.
    const auto bytes = static_cast<size_t>(std::max<int64_t>(declare_parameter("bytes", 64), 16));
    const auto period = std::chrono::nanoseconds(static_cast<int64_t>(1e9 / hz));
    msgs_.resize(n);
    seq_.assign(n, 0);
    for (int i = 0; i < n; ++i) {
      msgs_[i].data.assign(bytes, 0);
      pubs_.push_back(create_publisher<Msg>(topic(out, i), rclcpp::QoS(10)));
      timers_.push_back(create_wall_timer(period, [this, i] {
        const uint64_t stamp[2] = {now_ns(CLOCK_MONOTONIC), ++seq_[i]};
        std::memcpy(msgs_[i].data.data(), stamp, sizeof stamp);
        pubs_[i]->publish(msgs_[i]);
      }));
    }
  }

private:
  std::vector<Msg> msgs_;
  std::vector<uint64_t> seq_;
  std::vector<rclcpp::Publisher<Msg>::SharedPtr> pubs_;
  std::vector<rclcpp::TimerBase::SharedPtr> timers_;
};

class Relay : public rclcpp::Node
{
public:
  explicit Relay(const rclcpp::NodeOptions & o) : Node("tc_relay", o)
  {
    const int n = declare_parameter("topics", 4);
    const auto in = declare_parameter("in", std::string("/tc/a"));
    const auto out = declare_parameter("out", std::string("/tc/b"));
    for (int i = 0; i < n; ++i) {
      pubs_.push_back(create_publisher<Msg>(topic(out, i), rclcpp::QoS(10)));
      subs_.push_back(create_subscription<Msg>(
        topic(in, i), rclcpp::QoS(10), [this, i](const Msg & m) { pubs_[i]->publish(m); }));
    }
  }

private:
  std::vector<rclcpp::Publisher<Msg>::SharedPtr> pubs_;
  std::vector<rclcpp::Subscription<Msg>::SharedPtr> subs_;
};

class Sink : public rclcpp::Node
{
public:
  explicit Sink(const rclcpp::NodeOptions & o) : Node("tc_sink", o)
  {
    const int n = declare_parameter("topics", 4);
    const auto in = declare_parameter("in", std::string("/tc/b"));
    tag_ = declare_parameter("tag", std::string("sink"));
    // Reserved up front: no allocation in the measured callback.
    const auto capacity = static_cast<size_t>(declare_parameter("capacity", 400000));
    rows_.resize(n);
    for (int i = 0; i < n; ++i) {
      rows_[i].reserve(capacity);
      subs_.push_back(
        create_subscription<Msg>(topic(in, i), rclcpp::QoS(10), [this, i](const Msg & m) {
          const uint64_t recv = now_ns(CLOCK_MONOTONIC);
          uint64_t stamp[2];
          std::memcpy(stamp, m.data.data(), sizeof stamp);
          if (rows_[i].size() < rows_[i].capacity()) {
            rows_[i].push_back({stamp[1], stamp[0], recv});
          }
        }));
    }
  }

  ~Sink() override
  {
    for (size_t i = 0; i < rows_.size(); ++i) {
      std::ofstream f(tag_ + "_t" + std::to_string(i) + ".csv");
      f << "seq,pub_ns,recv_ns\n";
      for (const auto & r : rows_[i]) {
        f << r.seq << ',' << r.pub << ',' << r.recv << '\n';
      }
    }
  }

private:
  struct Row
  {
    uint64_t seq, pub, recv;
  };
  std::string tag_;
  std::vector<std::vector<Row>> rows_;
  std::vector<rclcpp::Subscription<Msg>::SharedPtr> subs_;
};

class Burner : public rclcpp::Node
{
public:
  explicit Burner(const rclcpp::NodeOptions & o) : Node("burner", o)
  {
    const int period_ms = declare_parameter("period_ms", 20);
    const double busy_ms = declare_parameter("busy_ms", 1.0);
    const double sleep_ms = declare_parameter("sleep_ms", 0.0);
    const auto busy = static_cast<uint64_t>(busy_ms * 1e6);
    const auto sleep = std::chrono::nanoseconds(static_cast<int64_t>(sleep_ms * 1e6));
    timer_ = create_wall_timer(std::chrono::milliseconds(period_ms), [busy, sleep] {
      // Thread CPU time, not wall time: preemption does not shorten the CPU it uses.
      const uint64_t end = now_ns(CLOCK_THREAD_CPUTIME_ID) + busy;
      while (now_ns(CLOCK_THREAD_CPUTIME_ID) < end) {
      }
      std::this_thread::sleep_for(sleep);
    });
  }

private:
  rclcpp::TimerBase::SharedPtr timer_;
};

}  // namespace orma_integration

RCLCPP_COMPONENTS_REGISTER_NODE(orma_integration::Pub)
RCLCPP_COMPONENTS_REGISTER_NODE(orma_integration::Relay)
RCLCPP_COMPONENTS_REGISTER_NODE(orma_integration::Sink)
RCLCPP_COMPONENTS_REGISTER_NODE(orma_integration::Burner)
