#!/usr/bin/env bash
# Download the test ROM suites used by gb-core's integration tests.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p test-roms
cd test-roms

if [ ! -d blargg ]; then
    curl -sL https://github.com/retrio/gb-test-roms/archive/refs/heads/master.tar.gz | tar xz
    mv gb-test-roms-master blargg
fi
[ -f dmg-acid2.gb ] || curl -sL -o dmg-acid2.gb \
    https://github.com/mattcurrie/dmg-acid2/releases/download/v1.0/dmg-acid2.gb
[ -f cgb-acid2.gbc ] || curl -sL -o cgb-acid2.gbc \
    https://github.com/mattcurrie/cgb-acid2/releases/download/v1.1/cgb-acid2.gbc

if [ ! -d jsmolka ]; then
    curl -sL https://github.com/jsmolka/gba-tests/archive/refs/heads/master.tar.gz | tar xz
    mv gba-tests-master jsmolka
fi
echo "test ROMs ready"
