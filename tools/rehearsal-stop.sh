#!/bin/bash
# Stop the rehearsal. Both processes are detached on purpose — see rehearsal.sh.
lsof -ti:8519 -sTCP:LISTEN 2>/dev/null | xargs kill 2>/dev/null && echo "server stopped" || echo "no server on 8519"
pkill -f "target/release/pmg-node" 2>/dev/null && echo "node stopped" || echo "no node running"
