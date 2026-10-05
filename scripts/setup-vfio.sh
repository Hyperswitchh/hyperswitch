#!/bin/sh
# Reserve a GPU for passthrough. Dry run by default; pass --apply to write files.
#   sudo ./scripts/setup-vfio.sh 10de:2484 10de:228b --apply
# IDs come from: lspci -nn | grep -iE 'vga|audio'
set -eu

[ $# -ge 1 ] || { echo "usage: $0 <vendor:device>... [--apply]"; exit 2; }
apply=no; ids=""
for a in "$@"; do
  case "$a" in --apply) apply=yes ;; *) ids="${ids:+$ids,}$a" ;; esac
done

modprobe_conf="options vfio-pci ids=$ids
softdep nvidia pre: vfio-pci
softdep amdgpu pre: vfio-pci
softdep nouveau pre: vfio-pci"

echo "IOMMU groups containing your devices:"
for g in /sys/kernel/iommu_groups/*; do
  for d in "$g"/devices/*; do
    id=$(lspci -n -s "${d##*/}" | awk '{print $3}')
    case ",$ids," in *",$id,"*) echo "  group ${g##*/}: $(lspci -nns "${d##*/}")" ;; esac
  done
done
echo "(every device in the same group must be passed through together)"
echo
echo "/etc/modprobe.d/hyperswitch-vfio.conf:"
echo "$modprobe_conf" | sed 's/^/  /'

if [ "$apply" = yes ]; then
  echo "$modprobe_conf" > /etc/modprobe.d/hyperswitch-vfio.conf
  update-initramfs -u
  echo "Done. Reboot, then check: lspci -k | grep -A2 -i vga  (driver should be vfio-pci)"
else
  echo; echo "Dry run. Re-run with --apply to write it."
fi
