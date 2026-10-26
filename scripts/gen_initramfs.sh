#!/bin/sh
# Write a gen_init_cpio file list for the root filesystem to stdout.
#   usage: gen_initramfs.sh TEENY_BINARY ROOTFS_DIR
# The kernel build turns this list into the embedded initramfs, so no root
# privileges or cpio tool are needed.
set -eu
bin=$(realpath "$1")
root=$(realpath "$2")

echo "dir /dev 0755 0 0"
echo "nod /dev/console 0600 0 0 c 5 1"
for d in bin sbin proc sys tmp etc root home var; do
	echo "dir /$d 0755 0 0"
done
echo "file /bin/teeny $bin 0755 0 0"
echo "slink /init /bin/teeny 0777 0 0"
echo "slink /sbin/init /bin/teeny 0777 0 0"

# Every applet the binary reports becomes a symlink in /bin.
for a in $("$bin" | tail -n 1); do
	[ "$a" = init ] && continue
	echo "slink /bin/$a teeny 0777 0 0"
done

# Ship rootfs/ as-is: directories first, then files.
cd "$root"
find . -mindepth 1 -type d | sort | while read -r d; do
	case " bin sbin proc sys tmp etc root home var " in
	*" ${d#./} "*) continue ;;
	esac
	echo "dir /${d#./} 0755 0 0"
done
find . -type f | sort | while read -r f; do
	mode=644
	[ -x "$f" ] && mode=755
	echo "file /${f#./} $root/${f#./} $mode 0 0"
done
