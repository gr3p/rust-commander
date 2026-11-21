#!/usr/bin/env python3
"""A pass-through FUSE file system that answers slowly or stops answering. It stands in for an SMB or NFS share when
testing the disk usage count: its type is fuse.slowfs, which the program treats as a network disk.

    python3 scripts/slowfs.py SOURCE MOUNTPOINT [--delay-ms 5] [--hang-after N --hang-secs S]

Needs fusepy and libfuse 2:  python3 -m venv /tmp/fusevenv && /tmp/fusevenv/bin/pip install fusepy
Stop it with:  fusermount -u MOUNTPOINT
"""
import argparse
import errno
import os
import threading
import time

from fuse import FUSE, FuseOSError, Operations


class SlowFs(Operations):
    def __init__(self, source, delay, hang_after, hang_secs):
        self.source = os.path.realpath(source)
        self.delay = delay
        self.hang_after = hang_after
        self.hang_secs = hang_secs
        self.calls = 0
        self.blocked_until = 0.0
        self.lock = threading.Lock()

    def gate(self):
        # After hang_after calls the whole server goes quiet for hang_secs, like a share whose network dropped.
        with self.lock:
            self.calls += 1
            if self.hang_after and self.calls == self.hang_after:
                self.blocked_until = time.time() + self.hang_secs
            wait = self.blocked_until - time.time()
        if wait > 0:
            time.sleep(wait)
        if self.delay:
            time.sleep(self.delay)

    def real(self, path):
        return os.path.join(self.source, path.lstrip("/"))

    def getattr(self, path, fh=None):
        self.gate()
        try:
            st = os.lstat(self.real(path))
        except OSError as err:
            raise FuseOSError(err.errno)
        keys = ("st_atime", "st_ctime", "st_gid", "st_mode", "st_mtime", "st_nlink", "st_size", "st_uid", "st_blocks")
        out = {key: getattr(st, key) for key in keys}
        return out

    def readdir(self, path, fh):
        self.gate()
        try:
            names = os.listdir(self.real(path))
        except OSError as err:
            raise FuseOSError(err.errno)
        return [".", ".."] + names

    def readlink(self, path):
        self.gate()
        return os.readlink(self.real(path))

    def access(self, path, mode):
        self.gate()
        if not os.access(self.real(path), mode):
            raise FuseOSError(errno.EACCES)

    def statfs(self, path):
        self.gate()
        st = os.statvfs(self.real(path))
        return {key: getattr(st, key) for key in ("f_bavail", "f_bfree", "f_blocks", "f_bsize", "f_favail", "f_ffree", "f_files", "f_flag", "f_frsize", "f_namemax")}

    def open(self, path, flags):
        self.gate()
        return os.open(self.real(path), flags)

    def read(self, path, size, offset, fh):
        os.lseek(fh, offset, os.SEEK_SET)
        return os.read(fh, size)

    def release(self, path, fh):
        os.close(fh)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("source")
    parser.add_argument("mountpoint")
    parser.add_argument("--delay-ms", type=float, default=5.0)
    parser.add_argument("--hang-after", type=int, default=0)
    parser.add_argument("--hang-secs", type=float, default=30.0)
    args = parser.parse_args()
    fs = SlowFs(args.source, args.delay_ms / 1000.0, args.hang_after, args.hang_secs)
    FUSE(fs, args.mountpoint, foreground=True, nothreads=False, ro=True, fsname="slowfs", subtype="slowfs")


if __name__ == "__main__":
    main()
