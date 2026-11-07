from orma_client import view


def cb(pid, handle, node, kind, topic=None, service=None, symbol=None):
    return {
        "id": f"c:{pid}:1:ros:{handle}",
        "node": node,
        "kind": kind,
        "topic": topic,
        "service": service,
        "symbol": symbol,
    }


def window():
    talker, listener = "n:10:1:a0", "n:20:1:c0"
    return {
        "processes": [
            {"pid": 10, "cold": False},
            {"pid": 20, "cold": False},
            {"pid": 30, "cold": True},
        ],
        "nodes": [
            {"id": talker, "fqn": "/talker", "pid": 10},
            {"id": listener, "fqn": "/ns/listener", "pid": 20},
        ],
        "topics": [
            {
                "id": "topic:ros:/chatter",
                "name": "/chatter",
                "publishers": [talker],
                "subscribers": [listener],
            },
            {
                "id": "topic:ros:/rosout",
                "name": "/rosout",
                "publishers": [talker, listener],
                "subscribers": [],
            },
            {"id": "topic:ros:/odom", "name": "/odom", "publishers": [], "subscribers": []},
            {
                "id": "topic:ros:/parameter_events",
                "name": "/parameter_events",
                "publishers": [],
                "subscribers": [listener],
            },
        ],
        "callbacks": [
            cb(10, "aaaa00000001", talker, "timer", symbol="Talker::tick()"),
            cb(10, "aaaa00000002", talker, "service", service="/talker/get_parameters"),
            cb(20, "bbbb00000001", listener, "sub", "topic:ros:/chatter", symbol="on_msg"),
            cb(20, "bbbb00000002", listener, "sub", "topic:ros:/chatter", symbol="on_msg"),
            cb(20, "bbbb00000003", listener, "sub", "topic:ros:/parameter_events"),
            cb(30, "cccc00a7e448", None, "unknown"),
            cb(20, "bbbb00000004", None, "sub", "topic:ros:/odom"),
        ],
        "subscriptions": [
            {"topic": "topic:ros:/chatter", "node": listener},
            {"topic": "topic:ros:/odom", "node": "p:30:1"},
        ],
        "threads": [{"id": "t:10:1:11"}, {"id": "t:20:1:21"}, {"id": "t:30:1:31"}],
    }


def ids(entities):
    return [e["id"] for e in entities]


def test_internal_topics_and_services_are_hidden_unless_asked():
    w = window()
    shown = view.visible(w)
    assert ids(shown["topics"]) == ["topic:ros:/chatter", "topic:ros:/odom"]
    assert "c:10:1:ros:aaaa00000002" not in ids(shown["callbacks"])
    assert "c:20:1:ros:bbbb00000003" not in ids(shown["callbacks"])
    assert len(view.visible(w, infra=True)["callbacks"]) == len(w["callbacks"])


def test_labels_name_the_kind_and_target_or_function():
    labels = view.callback_labels(window())
    assert labels["c:10:1:ros:aaaa00000001"] == "timer Talker::tick()"
    assert labels["c:10:1:ros:aaaa00000002"] == "srv(/talker/get_parameters)"
    assert labels["c:20:1:ros:bbbb00000001"] == "sub(/chatter) #000001"
    assert labels["c:20:1:ros:bbbb00000002"] == "sub(/chatter) #000002"
    assert labels["c:30:1:ros:cccc00a7e448"] == "<cb 0xcccc00a7e448>"
    assert labels["c:20:1:ros:bbbb00000004"] == "sub(/odom)"


def test_callbacks_without_a_node_come_last_and_say_cold():
    w = window()
    order = ids(view.callbacks_in_order(w))
    assert order[-2:] == ["c:20:1:ros:bbbb00000004", "c:30:1:ros:cccc00a7e448"]
    assert view.owner_label(w, "n:20:1:c0") == "/ns/listener"
    assert view.owner_label(w, "c:20:1:ros:bbbb00000004") == "unlinked"
    assert view.owner_label(w, "c:30:1:ros:cccc00a7e448") == "unlinked (cold)"


def test_a_node_name_shows_that_node_and_the_topics_it_uses():
    shown = view.visible(window(), {"pids": [20], "node": "/ns/.*"})
    assert ids(shown["nodes"]) == ["n:20:1:c0"]
    assert ids(shown["topics"]) == ["topic:ros:/chatter"]
    assert ids(shown["callbacks"]) == [
        "c:20:1:ros:bbbb00000001",
        "c:20:1:ros:bbbb00000002",
        "c:20:1:ros:bbbb00000004",
    ]
    assert [p["pid"] for p in shown["processes"]] == [20]
    assert ids(shown["threads"]) == ["t:20:1:21"]


def test_a_topic_name_shows_both_ends():
    shown = view.visible(window(), {"pids": [10, 20], "topic": "/chatter"})
    assert ids(shown["nodes"]) == ["n:10:1:a0", "n:20:1:c0"]
    assert ids(shown["topics"]) == ["topic:ros:/chatter"]
    assert shown["subscriptions"] == [{"topic": "topic:ros:/chatter", "node": "n:20:1:c0"}]


def test_exclude_leaves_out_what_it_names():
    shown = view.visible(window(), {"pids": [10, 20, 30], "exclude": "/talker"})
    assert ids(shown["nodes"]) == ["n:20:1:c0"]
    assert "c:10:1:ros:aaaa00000001" not in ids(shown["callbacks"])
    assert "c:30:1:ros:cccc00a7e448" in ids(shown["callbacks"])


def test_exclude_alone_over_every_process_keeps_their_unlinked_callbacks():
    shown = view.visible(window(), {"all": True, "pids": [], "exclude": "/talker"})
    assert ids(shown["nodes"]) == ["n:20:1:c0"]
    assert "c:30:1:ros:cccc00a7e448" in ids(shown["callbacks"])


def graph(flows=()):
    ids = {name: f"n:{i}" for i, name in enumerate(["/cam", "/det", "/depth", "/map", "/ctl"])}

    def topic(name, publisher, *subscribers):
        return {
            "id": f"t{name}",
            "name": name,
            "publishers": [ids[publisher]],
            "subscribers": [ids[s] for s in subscribers],
        }

    return {
        "nodes": [{"id": i, "fqn": name} for name, i in ids.items()],
        "topics": [
            topic("/image", "/cam", "/det", "/depth"),
            topic("/objects", "/det", "/ctl"),
            topic("/points", "/depth", "/map"),
            topic("/grid", "/map", "/ctl"),
            topic("/back", "/ctl", "/cam"),
        ],
        "flows": [
            {"node": ids[n], "input": f"t{i}", "output": f"t{o}", "kind": k, "ambiguous": a}
            for n, i, o, k, a in flows
        ],
    }


def test_paths_are_every_way_shortest_first():
    paths, complete = view.find_paths(graph(), "/cam", "/ctl")
    assert complete
    assert [p["topics"] for p in paths] == [
        ["/image", "/objects"],
        ["/image", "/points", "/grid"],
    ]
    assert paths[0]["nodes"] == ["/cam", "/det", "/ctl"]
    assert [p["unknown"] for p in paths] == [1, 2]


def test_a_measured_path_comes_before_a_shorter_unknown_one():
    flows = [
        ("/depth", "/image", "/points", "sync", 0),
        ("/map", "/points", "/grid", "stamp", 0),
    ]
    paths, _ = view.find_paths(graph(flows), "/cam", "/ctl")
    assert [p["unknown"] for p in paths] == [0, 1]
    assert paths[0]["nodes"][1] == "/depth"


def test_an_ambiguous_or_blind_flow_is_no_link():
    for flow in [
        ("/det", "/image", "/objects", "sync", 3),
        ("/det", "/image", "/objects", "blind", 0),
    ]:
        paths, _ = view.find_paths(graph([flow]), "/cam", "/ctl", max_nodes=3)
        assert paths[0]["unknown"] == 1


def test_paths_narrow():
    w = graph()
    assert len(view.find_paths(w, "/cam", "/ctl", max_nodes=3)[0]) == 1
    assert view.find_paths(w, "/cam", "/ctl", via=("/map",))[0][0]["topics"][-1] == "/grid"
    assert len(view.find_paths(w, "/cam", "/ctl", skip_nodes=("/det",))[0]) == 1
    assert len(view.find_paths(w, "/cam", "/ctl", skip_topics=("/grid",))[0]) == 1
    assert view.find_paths(w, "/cam", "/nosuch")[0] == []


def test_a_cycle_ends_and_too_many_paths_say_so(monkeypatch):
    assert view.find_paths(graph(), "/ctl", "/map")[0][0]["nodes"][-1] == "/map"
    monkeypatch.setattr(view, "MAX_PATHS", 1)
    paths, complete = view.find_paths(graph(), "/cam", "/ctl")
    assert len(paths) == 1
    assert not complete


def test_a_long_way_is_found_past_a_large_dead_end(monkeypatch):
    # /cam also feeds a ladder that never reaches /ctl: its paths double each rung.
    w = graph()
    rungs = 40
    for i in range(rungs):
        w["nodes"] += [{"id": f"a{i}", "fqn": f"/a{i}"}, {"id": f"b{i}", "fqn": f"/b{i}"}]
    ids = [f"n:{i}" for i in range(5)]
    w["topics"].append(
        {"id": "s", "name": "/s", "publishers": [ids[0]], "subscribers": ["a0", "b0"]}
    )
    for i in range(rungs - 1):
        out = [f"a{i + 1}", f"b{i + 1}"]
        for n in (f"a{i}", f"b{i}"):
            w["topics"].append({"id": n, "name": f"/{n}", "publishers": [n], "subscribers": out})
    monkeypatch.setattr(view, "MAX_STEPS", 1000)
    paths, complete = view.find_paths(w, "/cam", "/ctl", max_nodes=64)
    assert complete
    assert len(paths) == 2
