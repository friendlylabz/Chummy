# /// script
# requires-python = ">=3.14"
# dependencies = ["tzdata==2026.6"]
#
# [tool.chummy]
# name = "current_time"
# title = "Current date and time"
# version = "1.0.1"
# ///
"""Tells the model today's date and the time."""

import datetime
import os
import zoneinfo

import chummy


@chummy.tool
def current_time(timezone: str = "") -> str:
    """Get the current date and time.

    timezone: an IANA time zone like Europe/Paris; leave empty for the user's own
    """
    name = timezone.strip() or os.environ.get("TZ", "") or "UTC"
    try:
        zone = zoneinfo.ZoneInfo(name)
    except (zoneinfo.ZoneInfoNotFoundError, ValueError):
        raise chummy.ToolError(f"Unknown time zone: {name}. Use a name like America/New_York.")
    now = datetime.datetime.now(zone)
    return now.strftime(f"%A %d %B %Y, %H:%M ({name}, UTC%z)")
