"""The ArmoTrace launch action."""

from launch.action import Action
from launch.event_handlers import OnShutdown
from launch.logging import get_logger

import armo_client

CLIENT = "armo_launch"


class ArmoTrace(Action):
    """Gives the launch's later processes the `armo env` variables of `method`.

    With `record`, starts a recording when the action runs; armo-collectd owns it, and it is
    stopped at launch shutdown if `stop_on_shutdown`. The other keywords are the `armo record`
    options under their record_start field names; `node`, `topic` and `exclude` raise ArmoError
    until armo-collectd applies them.
    """

    def __init__(
        self,
        *,
        method="preload",
        record=False,
        stop_on_shutdown=True,
        node=None,
        topic=None,
        exclude=None,
        **options,
    ):
        super().__init__()
        self.method = method
        self.record = record
        self.stop_on_shutdown = stop_on_shutdown
        given = node is not None or topic is not None or exclude is not None
        targets = armo_client.targets(False, (), node, topic, exclude) if given else None
        self.fields = armo_client.record_start(method, targets, **options)
        self.record_id = None
        self.logger = get_logger("armo")

    def execute(self, context):
        # Before the environment changes, so a collectd this starts gets none of it.
        if self.record:
            with armo_client.connect(CLIENT) as c:
                reply = c.request("record_start", **self.fields)
            self.record_id = reply["record_id"]
            self.logger.info(f"recording {self.record_id} in {reply['dir']}")
            if self.stop_on_shutdown:
                context.register_event_handler(OnShutdown(on_shutdown=self._stop))
        context.environment.update(armo_client.environment(self.method, context.environment))
        return None

    def _stop(self, event, context):
        with armo_client.connect(CLIENT, start=False) as c:
            c.request("record_stop", record_id=self.record_id)
        self.logger.info(f"stopped {self.record_id}")
        return None
