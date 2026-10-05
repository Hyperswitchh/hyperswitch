#!/bin/sh
# Runs inside the image at build time.
set -eu

# kernel modules needed at boot
printf '%s\n' vfio vfio_iommu_type1 vfio_pci kvmfr uinput > /etc/modules-load.d/hyperswitch.conf

# minimal seat user that auto-logs into the switcher session
useradd -m -G input,kvm,libvirt,video,render hs || true

systemctl enable libvirtd.service
systemctl enable hyperswitchd.service
systemctl enable hyperswitch-session.service
systemctl set-default graphical.target
