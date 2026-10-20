from setuptools import setup

package_name = "orma_cli"

setup(
    name=package_name,
    version="0.0.0",
    packages=[package_name, "orma_client"],
    data_files=[
        ("share/ament_index/resource_index/packages", ["resource/" + package_name]),
        ("share/" + package_name, ["package.xml"]),
        (
            "share/" + package_name + "/environment",
            ["environment/orma-argcomplete.bash", "environment/orma-argcomplete.zsh"],
        ),
    ],
    install_requires=["setuptools"],
    zip_safe=True,
    maintainer="nevlife",
    maintainer_email="nevlife000720@gmail.com",
    description="The orma command, the orma-collectd client and the OrmaTrace launch action.",
    license="MIT",
    extras_require={"test": ["pytest"]},
    entry_points={"console_scripts": ["orma = orma_cli.cli:main"]},
)
