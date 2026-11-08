#!/usr/bin/env bash
# The demo: one GCS scenario, the same on every camera, through the platform's HTTP API.
#
#   ./demo.sh                          starts the three simulators and a platform for each,
#                                      runs the scenario on every camera, then stops them all
#   ./demo.sh http://127.0.0.1:8080    runs the scenario on a platform that's already running
#   ./demo.sh --metrics                like the first, with the development build serving
#                                      metrics on ports 9000-9002 for ../monitoring/; afterwards
#                                      it keeps sending traffic until Ctrl-C
#
# Needs curl and jq. It uses nothing but the API, so it does exactly what any GCS could do.

set -euo pipefail
cd "$(dirname "$0")"

# Colours only when the output is a terminal, not when it goes to a file.
if [[ -t 1 ]]; then
  bold=$'\e[1m' dim=$'\e[2m' green=$'\e[32m' red=$'\e[31m' reset=$'\e[0m'
else
  bold='' dim='' green='' red='' reset=''
fi

# Response bodies, and the logs of everything this script starts.
work=$(mktemp -d)

# call METHOD PATH [BODY]: sends one request to the platform at $api, then prints the request
# and a one-line summary of the answer, with how long the answer took.
call() {
  local method=$1 path=$2 body=${3:-}
  # The `\n` matters: without a line ending, `read` below reports failure and `set -e` stops.
  local args=(-s -o "$work/answer" -w '%{http_code} %{time_total}\n' -X "$method")
  if [[ -n $body ]]; then
    args+=(-H 'Content-Type: application/json' -d "$body")
  fi
  # `${body:+ $body}` is a space and the body when there is one, and nothing otherwise.
  printf '%s→ %-4s %s%s%s\n' "$dim" "$method" "$path" "${body:+ $body}" "$reset"
  local status seconds color summary
  read -r status seconds < <(curl "${args[@]}" "$api$path")
  summary=$(jq -r '
    if .error then "\(.error.code): \(.error.message)"
    elif .properties then (.properties | to_entries | map("\(.key) = \(.value.value)") | join(", "))
    else "value = \(.value)" end' "$work/answer")
  if [[ $status == 2* ]]; then color=$green; else color=$red; fi
  printf '  %s%s%s  %s  %s(%s ms)%s\n' "$color" "$status" "$reset" "$summary" "$dim" \
    "$(milliseconds "$seconds")" "$reset"
}

# milliseconds SECONDS: turns curl's "0.104123" into "104.1", with whole numbers only, so it
# works whatever decimal separator the computer's language settings use.
milliseconds() {
  local whole=${1%.*} fraction="${1#*.}000000"
  local tenths=$(( (10#$whole * 1000000 + 10#${fraction:0:6}) / 100 ))
  echo "$(( tenths / 10 )).$(( tenths % 10 ))"
}

# step TITLE: starts a new part of the scenario.
step() {
  printf '\n%s%s%s\n' "$bold" "$1" "$reset"
}

# wait_until_online URL: waits up to 10 s for the platform at URL to report the camera online.
wait_until_online() {
  for _ in $(seq 50); do
    if [[ $(curl -s "$1/v1/payload" | jq -r '.connection.status' 2>/dev/null) == online ]]; then
      return
    fi
    sleep 0.2
  done
  echo "${red}The platform at $1 isn't online after 10 s.${reset} Logs: $work" >&2
  exit 1
}

# scenario: the GCS scenario, run against the platform at $api. Every limit it uses comes from
# the camera's own description, so the same steps work on every camera.
scenario() {
  local payload maximum last_palette
  payload=$(curl -s "$api/v1/payload")
  jq -r --arg bold "$bold" --arg reset "$reset" '
    .identity as $id | .properties as $p |
    "\($bold)\($id.vendor) \($id.model)\($reset) at \($id.uri): firmware \($id.firmware // "unknown"), driver \($id.driver)\(if $id.profile then ", profile \($id.profile)" else "" end)",
    "zoom \($p.zoom.minimum)–\($p.zoom.maximum)x in steps of \($p.zoom.resolution); sensors: \($p.sensor.options | join(", "))",
    "palettes: \($p.thermal_palette.options | join(", "))"' <<<"$payload"
  maximum=$(jq '.properties.zoom.maximum' <<<"$payload")
  last_palette=$(jq -r '.properties.thermal_palette.options[-1]' <<<"$payload")

  step "Zoom to 4x, then one step in and one out"
  call PUT /v1/payload/properties/zoom '{"value": 4}'
  call POST /v1/payload/actions/zoom_step '{"direction": "in"}'
  call POST /v1/payload/actions/zoom_step '{"direction": "out"}'

  step "Zoom to 4.6x: rounded to what this camera can do"
  call PUT /v1/payload/properties/zoom '{"value": 4.6}'

  step "At the end of the range, a step changes nothing"
  call PUT /v1/payload/properties/zoom "{\"value\": $maximum}"
  call POST /v1/payload/actions/zoom_step '{"direction": "in"}'

  step "Out of range: refused before anything reaches the camera"
  call PUT /v1/payload/properties/zoom "{\"value\": $(jq -n "$maximum + 1")}"

  step "Thermal sensor, then back to visible"
  call PUT /v1/payload/properties/sensor '{"value": "thermal"}'
  call PUT /v1/payload/properties/sensor '{"value": "visible"}'

  step "This camera's last palette, then lava, which only the LC-30T has"
  call PUT /v1/payload/properties/thermal_palette "{\"value\": \"$last_palette\"}"
  call PUT /v1/payload/properties/thermal_palette '{"value": "lava"}'

  step "A typo in the body"
  call PUT /v1/payload/properties/zoom '{"valeu": 4}'

  step "Reads come from the cache, which every write keeps up to date"
  call GET /v1/payload/properties/zoom
}

# burst API: 30 writes at the same moment. The core runs one and queues 16; the others get
# 503 busy at once, which the dashboard shows.
burst() {
  local curls=() i
  for i in $(seq 30); do
    curl -s -o /dev/null -X PUT -H 'Content-Type: application/json' \
      -d "{\"value\": $(( i % 2 ? 3 : 6 ))}" "$1/v1/payload/properties/zoom" &
    curls+=("$!")
  done
  # Only these: a plain `wait` would also wait for the simulators and platforms.
  wait "${curls[@]}"
}

# `--metrics`: the development build, with metrics (README, "Metrics and Grafana").
metrics=
if [[ ${1:-} == --metrics ]]; then
  metrics=1
  shift
fi

# One platform that's already running: just run the scenario on it.
if [[ $# -gt 0 ]]; then
  api=$1
  wait_until_online "$api"
  scenario
  exit
fi

# Otherwise start everything: the three simulators, and a platform for each camera.
echo "Building the platform and the simulators..."
# `${metrics:+...}` is the text after `+` when `metrics` is set, and nothing otherwise. It's
# left unquoted on purpose, here and below, so it becomes two words, or none.
cargo build --quiet ${metrics:+--features prometheus}
cargo build --release --quiet --manifest-path ../camera-simulations/Cargo.toml

pids=()
# Stop everything this script started, however it ends.
trap 'kill "${pids[@]}" 2>/dev/null; wait 2>/dev/null' EXIT

# start NAME COMMAND...: runs COMMAND in the background, with its output in $work/NAME.log.
start() {
  local name=$1
  shift
  "$@" >"$work/$name.log" 2>&1 &
  pids+=("$!")
}

for camera in bx20 vlr10 lc30t; do
  start "$camera-simulator" ../camera-simulations/target/release/camera-sim "$camera"
done
# With --metrics, the metrics listen on every address (0.0.0.0), so Prometheus can reach
# them from inside Docker.
start bx20-platform target/debug/platform --camera mavlink://127.0.0.1:14580 \
  --listen 127.0.0.1:8080 ${metrics:+--metrics-listen 0.0.0.0:9000}
start vlr10-platform target/debug/platform --camera mavlink://127.0.0.1:14581 \
  --listen 127.0.0.1:8081 ${metrics:+--metrics-listen 0.0.0.0:9001}
start lc30t-platform target/debug/platform --camera lc30t://127.0.0.1:9003 \
  --listen 127.0.0.1:8082 ${metrics:+--metrics-listen 0.0.0.0:9002}

apis=(http://127.0.0.1:8080 http://127.0.0.1:8081 http://127.0.0.1:8082)
for api in "${apis[@]}"; do
  wait_until_online "$api"
  printf '\n%s━━ %s ━━%s\n' "$bold" "$api" "$reset"
  scenario
done
echo
echo "Logs of the simulators and platforms: $work"

# With --metrics, keep the dashboard busy until Ctrl-C.
if [[ -n $metrics ]]; then
  echo
  echo "${bold}Metrics${reset} at http://localhost:9000/metrics (and 9001, 9002)."
  echo "Dashboard at http://localhost:3000, once monitoring/ runs:"
  echo "  docker compose -f ../monitoring/docker-compose.yml up -d"
  echo "Sending traffic until Ctrl-C: the scenario on every camera, then a burst on one."
  round=0
  while true; do
    for api in "${apis[@]}"; do
      # In a subshell, so a camera you stop on purpose doesn't end the script.
      (scenario) >/dev/null 2>&1 || true
    done
    burst "${apis[round % 3]}"
    round=$(( round + 1 ))
    sleep 2
  done
fi
