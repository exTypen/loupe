#!/bin/sh
osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $1) to true" >/dev/null
