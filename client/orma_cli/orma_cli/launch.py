"""The OrmaTrace launch action."""

from launch.action import Action
from launch.event_handlers import OnShutdown
from launch.logging import get_logger

import orma_client

CLIENT = "orma_launch"


class OrmaTrace(Action):
    """Gives the launch's later processes the `orma env` variables of `method`.

    With `record`, starts a recording when the action runs; orma-collectd owns it, and it is
    stopped at launch shutdown if `stop_on_shutdown`. The other keywords are the `orma record`
    options under their record_start field names; `node`, `topic` and `exclude` raise OrmaError
    until orma-collectd applies them.
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
        targets = orma_client.targets(False, (), node, topic, exclude) if given else None
        self.fields = orma_client.record_start(method, targets, **options)
        self.record_id = None
        self.logger = get_logger("orma")

    def execute(self, context):
        # Before the environment changes, so a collectd this starts gets none of it.
        if self.record:
            with orma_client.connect(CLIENT) as c:
                reply = c.request("record_start", **self.fields)
            self.record_id = reply["record_id"]
            self.logger.info(f"recording {self.record_id} in {reply['dir']}")
            if self.stop_on_shutdown:
                context.register_event_handler(OnShutdown(on_shutdown=self._stop))
        context.environment.update(orma_client.environment(self.method, context.environment))
        return None

    def _stop(self, event, context):
        with orma_client.connect(CLIENT, start=False) as c:
            c.request("record_stop", record_id=self.record_id)
        self.logger.info(f"stopped {self.record_id}")
        return None
