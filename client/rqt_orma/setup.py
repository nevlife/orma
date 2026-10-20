from setuptools import setup

package_name = "rqt_orma"

setup(
    name=package_name,
    version="0.0.0",
    packages=[package_name],
    data_files=[
        ("share/ament_index/resource_index/packages", ["resource/" + package_name]),
        ("share/" + package_name, ["package.xml", "plugin.xml"]),
    ],
    install_requires=["setuptools"],
    zip_safe=True,
    maintainer="nevlife",
    maintainer_email="nevlife000720@gmail.com",
    description="The orma GUI as an rqt plugin.",
    license="MIT",
    extras_require={"test": ["pytest"]},
    entry_points={"console_scripts": ["rqt_orma = rqt_orma.plugin:main"]},
)
