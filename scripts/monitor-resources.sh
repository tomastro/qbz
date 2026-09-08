#!/bin/bash
# Monitor QBZ resource usage
# Usage: ./scripts/monitor-resources.sh

echo "=== QBZ Resource Monitor ==="
echo "Waiting for QBZ process..."
echo ""

# Wait for process to start (Slint binary is named "qbz")
while ! pgrep -x "qbz" > /dev/null 2>&1; do
    sleep 0.5
done

PID=$(pgrep -x "qbz" | head -1)
echo "Found QBZ PID: $PID"
echo ""
echo "Press Ctrl+C to stop monitoring"
echo "==========================================="
echo ""

# Monitor loop
while true; do
    if ! ps -p "$PID" > /dev/null 2>&1; then
        echo "QBZ process ended"
        exit 0
    fi

    # Get stats
    CPU=$(ps -p "$PID" -o %cpu= 2>/dev/null | tr -d ' ')
    MEM=$(ps -p "$PID" -o rss= 2>/dev/null | tr -d ' ')
    MEM_MB=$((MEM / 1024))
    THREADS=$(ps -p "$PID" -o nlwp= 2>/dev/null | tr -d ' ')

    # Print stats
    printf "\r[QBZ] CPU: %5s%% | RAM: %4dMB | Threads: %2s" \
        "$CPU" "$MEM_MB" "$THREADS"

    sleep 1
done
