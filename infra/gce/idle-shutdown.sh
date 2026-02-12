#!/usr/bin/env bash
# Idle-shutdown cron job. Checks /tmp/docling_last_activity mtime.
# If idle for more than 15 minutes, shuts down the instance.
# Intended to run via cron every 5 minutes.

ACTIVITY_FILE="/tmp/docling_last_activity"
IDLE_THRESHOLD=900  # 15 minutes in seconds

if [ ! -f "$ACTIVITY_FILE" ]; then
    echo "$(date): No activity file found, shutting down."
    /sbin/shutdown -h now
    exit 0
fi

LAST_MODIFIED=$(stat -c %Y "$ACTIVITY_FILE" 2>/dev/null || stat -f %m "$ACTIVITY_FILE" 2>/dev/null)
NOW=$(date +%s)
IDLE_SECONDS=$((NOW - LAST_MODIFIED))

if [ "$IDLE_SECONDS" -ge "$IDLE_THRESHOLD" ]; then
    echo "$(date): Idle for ${IDLE_SECONDS}s (threshold: ${IDLE_THRESHOLD}s). Shutting down."
    /sbin/shutdown -h now
else
    echo "$(date): Active (idle ${IDLE_SECONDS}s / ${IDLE_THRESHOLD}s threshold)."
fi
