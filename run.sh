#!/bin/sh
# Launch the app with the user-local GTK4/GStreamer sysroot on this machine.
cd "$(dirname "$0")"
. ./env.sh
exec cargo run --release -- "$@"
