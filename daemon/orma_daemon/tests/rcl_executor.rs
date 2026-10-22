//! The executor probe choice against real processes, through the path ormad learns a process by
//! (maps, then the files): an rclpy `RclExecutor` from the overhaul's overlay maps the
//! rcl_executor core, a `SingleThreadedExecutor` on the stock install does not. Needs both ROS
//! installs, so it runs only when asked:
//!
//!     ORMA_TEST_OVERLAY=$HOME/ros2_ws/install/setup.bash \
//!         scripts/cargo.sh test --release -p ormad --test rcl_executor -- --ignored

use std::{
    env,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, Command, Stdio},
};

use orma_proto::msg::ExecutorProbes;
use ormad::{
    libs::{self, Lib},
    procs,
};

/// A python3 that has made its executor and waits for a line on stdin.
struct Python(Child);

impl Python {
    fn start(setup: &str, executor: &str) -> Python {
        let code = format!(
            "import sys, rclpy\n{executor}\nrclpy.init()\nexecutor = Executor()\n\
             print('ready', flush=True)\nsys.stdin.readline()\nrclpy.shutdown()\n"
        );
        let mut child = Command::new("bash")
            .args([
                "-c",
                r#"source "$0" >/dev/null && exec python3 -c "$1""#,
                setup,
            ])
            .arg(code)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("bash");
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready", "python3 under {setup} did not start");
        Python(child)
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Python {
    fn drop(&mut self) {
        let _ = self.0.stdin.as_mut().unwrap().write_all(b"\n");
        let _ = self.0.wait();
    }
}

/// The files of `pid` as `Kernel::learn` reads them, and the choice ormad makes from them.
fn choose(pid: u32) -> (Vec<libs::LibFile>, ExecutorProbes) {
    let root = procs::root(pid);
    let files: Vec<libs::LibFile> = procs::mapped(pid)
        .expect("maps")
        .iter()
        .filter_map(|m| libs::read(m.lib, &root, &m.path).ok().map(|(_, f)| f))
        .collect();
    let choice = libs::executor_probes(&files);
    (files, choice)
}

#[test]
#[ignore = "needs the overhaul's overlay (ORMA_TEST_OVERLAY) and the stock ROS install"]
fn the_core_is_chosen_from_the_symbols_of_what_a_process_maps() {
    let overlay = env::var("ORMA_TEST_OVERLAY").expect("ORMA_TEST_OVERLAY: the overlay's setup");
    let distro = env::var("ROS_DISTRO").unwrap_or_else(|_| "jazzy".into());
    let stock = format!("/opt/ros/{distro}/setup.bash");
    assert!(Path::new(&stock).exists(), "{stock}");

    let overhaul = Python::start(
        &overlay,
        "from rclpy.experimental import RclExecutor as Executor",
    );
    let plain = Python::start(
        &stock,
        "from rclpy.executors import SingleThreadedExecutor as Executor",
    );

    let (files, choice) = choose(overhaul.pid());
    let core: Vec<&libs::LibFile> = files.iter().filter(|f| f.lib == Lib::RclExecutor).collect();
    assert_eq!(
        core.len(),
        1,
        "the overlay's rclpy maps one rcl_executor core"
    );
    assert_eq!(core[0].refused, None);
    assert_eq!(choice, ExecutorProbes::RclExecutor);

    let (files, choice) = choose(plain.pid());
    assert!(files.iter().all(|f| f.lib != Lib::RclExecutor));
    let rcl: Vec<&str> = files
        .iter()
        .filter(|f| f.lib == Lib::Rcl)
        .map(|f| f.mapped.as_str())
        .collect();
    assert_eq!(rcl, [format!("/opt/ros/{distro}/lib/librcl.so")]);
    assert_eq!(choice, ExecutorProbes::Stock);
}
