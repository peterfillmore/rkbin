"""WebSocket broadcast plumbing.

The poller publishes one message per completed poll; every connected
WebSocket client has its own bounded queue so a slow client can never block
the poller or the other clients (oldest messages are dropped on overflow).
"""
from __future__ import annotations

import asyncio
import logging

logger = logging.getLogger(__name__)

QUEUE_SIZE = 64


class Broadcaster:
    def __init__(self) -> None:
        self._subscribers: set[asyncio.Queue] = set()

    def subscribe(self) -> asyncio.Queue:
        queue: asyncio.Queue = asyncio.Queue(maxsize=QUEUE_SIZE)
        self._subscribers.add(queue)
        return queue

    def unsubscribe(self, queue: asyncio.Queue) -> None:
        self._subscribers.discard(queue)

    @property
    def subscriber_count(self) -> int:
        return len(self._subscribers)

    def publish(self, message: dict) -> None:
        for queue in self._subscribers:
            try:
                queue.put_nowait(message)
            except asyncio.QueueFull:
                try:
                    queue.get_nowait()  # drop the oldest, keep the freshest
                except asyncio.QueueEmpty:
                    pass
                try:
                    queue.put_nowait(message)
                except asyncio.QueueFull:
                    logger.warning("Dropping WebSocket message for slow client")
