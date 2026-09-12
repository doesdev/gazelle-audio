#!/bin/sh
for pkg in zlib1g-dev libssl-dev libffi-dev libbz2-dev liblzma-dev libsqlite3-dev libreadline-dev; do
  if dpkg -s "$pkg" >/dev/null 2>&1; then
    echo "OK   $pkg"
  else
    echo "MISS $pkg"
  fi
done
