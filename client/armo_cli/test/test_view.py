from armo_client import view


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
