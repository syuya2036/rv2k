#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p target
clang -Wall -Wextra -Werror -fblocks probe.c -framework vmnet -o target/probe
clang -Wall -Wextra -Werror -fblocks bridge.c -framework vmnet -o target/bridge
