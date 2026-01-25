---
title: "Improved Hardlink Pattern"
stage: 0 # 0: Strawman
feature: Performance
---

# RFC: Improved Hardlink Pattern for Image Unpacking

## 1. Summary

This RFC proposes optimizing the container startup process by replacing the naive recursive file copy with a **Hardlink** strategy (or Reflink where available). This will drastically reduce the time and disk space required to "unpack" a cached image into a runtime bundle.

## 2. Motivation

Currently, `locald` uses a "Native Platform" approach for CNB, meaning it unpacks OCI images onto the host filesystem to run them.

The current implementation (`locald-builder/src/image.rs`) performs a standard recursive copy (`cp -r`) from the image cache to the runtime bundle directory.

- **Performance**: Copying thousands of small files (typical in `node_modules` or system roots) is slow (IOPS bound).
- **Disk Usage**: It duplicates the data on disk for every running container instance.

Docker avoids this using OverlayFS. Since we are running on the host (often without root privileges for mounting OverlayFS), we need a user-space alternative.

## 3. Detailed Design

### 3.1 The Strategy

Instead of copying file content, we will create **Hardlinks** for regular files.

1.  **Source**: The "expanded" image layer in the local cache (e.g., `~/.local/share/locald/images/...`).
2.  **Destination**: The runtime bundle directory (e.g., `.locald/containers/<id>/rootfs`).
3.  **Mechanism**:
    - Iterate through the source directory.
    - If **Directory**: `mkdir` in destination.
    - If **File**: `link(src, dst)`.
    - If **Symlink**: Re-create the symlink (copy the link target).

### 3.2 Benefits

- **Speed**: Creating a hardlink is a metadata-only operation. It is nearly instantaneous regardless of file size.
- **Space**: Zero additional disk usage for file content.
- **Isolation**:
  - If the process _reads_ the file, it works as expected.
  - If the process _deletes_ the file (unlink), it only removes the link in the bundle, leaving the cache intact.
  - **Risk**: If the process _modifies_ the file content in place (e.g., `echo "foo" >> file`), it **will corrupt the cache** because they share the same inode.

### 3.3 Safety: Copy-on-Write (CoW) Emulation

To mitigate the corruption risk, we must ensure that the runtime environment treats these files as Read-Only or implements a user-space CoW.

**Option A: Read-Only Rootfs (Recommended)**
We configure the OCI runtime spec (`config.json`) to mount the rootfs as `readonly: true`.

- **Pros**: Safe. Cache cannot be corrupted.
- **Cons**: The app cannot write to the filesystem.
- **Mitigation**: We explicitly mount writable volumes (tmpfs or bind mounts) for directories the app _needs_ to write to (e.g., `/tmp`, `/run`, `/app/logs`).

**Option B: Reflink (CoW)**
On filesystems that support it (Btrfs, XFS, APFS on macOS), we use `reflink` (ioctl `FICLONERANGE`).

- **Pros**: True CoW. The app can write to files without affecting the cache.
- **Cons**: Not supported on ext4 (standard Linux) without specific patches/versions. Fallback required.

### 3.4 Implementation Plan

1.  **Update `locald-builder`**:
    - Modify `copy_dir_recursive` to attempt hardlinking first.
    - Handle cross-device link errors (fallback to copy).
2.  **Update OCI Spec**:
    - Set `root.readonly = true` in the generated `config.json`.
    - Add standard writable mounts (`/tmp`, `/dev/shm`, etc.).

## 4. Alternatives

- **OverlayFS (Rootless)**: Requires user namespaces and kernel support. Complex to set up reliably across distros.
- **FUSE Overlay**: User-space OverlayFS. Adds performance overhead and dependency on `fuse-overlayfs` binary.

## 5. Unresolved Questions

- **Writable App Dirs**: Some apps expect to write to their own code directory (e.g., generating lockfiles). A Read-Only rootfs breaks this.
  - _Solution_: We can bind-mount the app's working directory from the host (which is writable) over the read-only rootfs layer. This is effectively what we do for "Host-First" execution anyway.
