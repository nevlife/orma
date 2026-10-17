from setuptools import setup

package_name = "armo_cli"

setup(
    name=package_name,
    version="0.0.0",
    packages=[package_name, "armo_client"],
    data_files=[
        ("share/ament_index/resource_index/packages", ["resource/" + package_name]),
        ("share/" + package_name, ["package.xml"]),
        (
            "share/" + package_name + "/environment",
            ["environment/armo-argcomplete.bash", "environment/armo-argcomplete.zsh"],
        ),
    ],
    install_requires=["setuptools"],
    zip_safe=True,
    maintainer="nevlife",
    maintainer_email="nevlife000720@gmail.com",
    description="The armo command, the armo-collectd client and the ArmoTrace launch action.",
    license="MIT",
    extras_require={"test": ["pytest"]},
    entry_points={"console_scripts": ["armo = armo_cli.cli:main"]},
)
