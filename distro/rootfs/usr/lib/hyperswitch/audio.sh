#!/bin/sh
# audio.sh mute|unmute <libvirt-domain>
# Each guest's QEMU audio stream shows up in PipeWire named after its domain.
set -eu
action="$1"; domain="$2"
node=$(wpctl status | awk -v d="$domain" '$0 ~ d { gsub(/[^0-9]/, "", $1); print $1; exit }')
[ -n "$node" ] || exit 0          # guest has no audio stream yet
case "$action" in
  mute)   wpctl set-mute "$node" 1 ;;
  unmute) wpctl set-mute "$node" 0 ;;
esac
