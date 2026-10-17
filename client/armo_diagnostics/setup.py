from setuptools import setup

package_name = "armo_diagnostics"

setup(
    name=package_name,
    version="0.0.0",
    packages=[package_name],
    data_files=[
        ("share/ament_index/resource_index/packages", ["resource/" + package_name]),
        ("share/" + package_name, ["package.xml"]),
        ("share/" + package_name + "/config", ["config/rules.yaml"]),
        ("share/" + package_name + "/launch", ["launch/armo_diagnostics.launch.py"]),
    ],
    install_requires=["setuptools"],
    zip_safe=True,
    maintainer="nevlife",
    maintainer_email="nevlife000720@gmail.com",
    description="Publishes armo-collectd's windows as /diagnostics against threshold rules.",
    license="MIT",
    extras_require={"test": ["pytest"]},
    entry_points={"console_scripts": ["armo_diagnostics = armo_diagnostics.node:main"]},
)
