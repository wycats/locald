# AppArmor Profile for locald-shim
# This profile allows locald-shim to use unprivileged user namespaces
# which is required for running rootless containers.

abi <abi/4.0>,
include <tunables/global>

profile locald-shim /home/**/target/debug/locald-shim flags=(attach_disconnected,mediate_deleted) {
  include <abstractions/base>
  include <abstractions/nameservice>
  include <abstractions/user-tmp>

  # Allow unprivileged user namespaces
  userns,

  # Allow reading/writing to the bundle directory
  owner @{HOME}/** rw,
  
  # Allow mounting
  mount,
  remount,
  umount,
  pivot_root,
}
