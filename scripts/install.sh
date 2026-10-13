#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/build.sh
if pgrep -x RingSwitch >/dev/null; then
  echo "请先从菜单栏退出 RingSwitch，再重新安装。" >&2
  exit 1
fi
ditto dist/RingSwitch.app /Applications/RingSwitch.app
open /Applications/RingSwitch.app
