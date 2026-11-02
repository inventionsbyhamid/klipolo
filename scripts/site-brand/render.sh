#!/bin/sh
# Renders site/assets/og.png and the web manifest's icons, and stamps the brand version (VERSION) into the
# site's links. One command, from anywhere in the repo: scripts/site-brand/render.sh
# It runs headless Chromium in Playwright's Docker image (the Python package, matching the image's browsers,
# is installed into the container on each run), so run it where Docker runs, not on the Mac.
set -eu
repo=$(cd "$(dirname "$0")/../.." && pwd)
exec docker run --rm --ipc=host --user "$(id -u):$(id -g)" -e HOME=/tmp -v "$repo:/repo" -w /repo \
  mcr.microsoft.com/playwright/python:v1.55.0-noble sh -c '
    python3 -m pip install --quiet --disable-pip-version-check --break-system-packages --target /tmp/pw playwright==1.55.0 &&
    PYTHONPATH=/tmp/pw python3 scripts/site-brand/render.py'
