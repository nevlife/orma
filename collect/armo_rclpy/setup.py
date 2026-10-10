from setuptools import setup

package_name = "armo_rclpy"

setup(
    name=package_name,
    version="0.0.0",
    packages=[package_name],
    data_files=[
        ("share/ament_index/resource_index/packages", ["resource/" + package_name]),
        ("share/" + package_name, ["package.xml"]),
        ("share/" + package_name + "/autoload", ["autoload/usercustomize.py"]),
    ],
    install_requires=["setuptools"],
    zip_safe=True,
    maintainer="nevlife",
    maintainer_email="nevlife000720@gmail.com",
    description="Calls the ros_trace_* functions rclpy does not call.",
    license="MIT",
    extras_require={"test": ["pytest"]},
)
