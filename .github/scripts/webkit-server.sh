#!/usr/bin/env bash
# Starts Playwright's server in Playwright's own image, so that the tests of the Apple
# projects (`e2e:apple`) drive the WebKit of that image, with its libraries and fonts,
# from this checkout. It writes the two variables that make Playwright connect to it to
# the job's environment, or prints them where there is no job.
#
#   webkit-server.sh
#
# The image is Playwright's own for this version, the same on every machine. WebKit
# installed on a runner is another place: it has other fonts (the runner's sans-serif
# is DejaVu Sans, the image's WenQuanYi Zen Hei), and it took 20 minutes for the 96
# tests the image ran in 14.
set -euo pipefail

# The version of Playwright in apps/web/package.json. The image holds the browsers of
# exactly that version, and its digest is written here: change the three together.
VERSION=1.65.1
DIGEST=sha256:1507cbfc550a63a6b7396791ca4f00df17beb56342d2447b3f106c306f525579
NAME=fvoci-pw-webkit
PORT=18699

wanted=$(jq --raw-output '.devDependencies["@playwright/test"]' apps/web/package.json)
if [[ $wanted != "$VERSION" ]]; then
  echo "apps/web uses Playwright $wanted and this script starts the image of $VERSION:" \
    "write the new version and its image's digest in $0" >&2
  exit 1
fi

# The server itself is the package of that version from npm, fetched inside the
# container, which is given nothing of the checkout. The browser reaches the preview
# of this checkout through the tests' own connection (`<loopback>`).
docker run --detach --name "$NAME" --publish "127.0.0.1:$PORT:$PORT" \
  --init --ipc=host --user pwuser --workdir /home/pwuser \
  "mcr.microsoft.com/playwright:v$VERSION-noble@$DIGEST" \
  npx --yes "playwright@$VERSION" run-server --port "$PORT" --host 0.0.0.0

# By the clock: the log is read whole, then searched.
SECONDS=0
while ((SECONDS < 120)); do
  log=$(docker logs "$NAME" 2>&1)
  if [[ $log == *'Listening on'* ]]; then
    {
      echo "PW_TEST_CONNECT_WS_ENDPOINT=ws://127.0.0.1:$PORT/"
      echo "PW_TEST_CONNECT_EXPOSE_NETWORK=<loopback>"
    } >>"${GITHUB_ENV:-/dev/stdout}"
    exit 0
  fi
  sleep 1
done
docker logs "$NAME" >&2
echo "$NAME did not listen after 120 seconds" >&2
exit 1
