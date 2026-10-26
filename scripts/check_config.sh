#!/bin/sh
# Fail if any option requested in the fragment ($1) did not survive into the
# final kernel .config ($2), e.g. because a dependency was missing. When an
# option appears more than once (a later layer overriding an earlier one),
# the last value wins, as it does for merge_config.sh.
set -eu
bad=$(grep -E '^CONFIG_[A-Z0-9_]+=' "$1" | awk -F= '{v[$1]=$0} END {for (k in v) print v[k]}' |
while IFS= read -r line; do
	opt=${line%%=*}
	want=${line#*=}
	if [ "$want" = n ]; then
		if grep -q "^$opt=y" "$2"; then echo "kernel config: $opt is still enabled"; fi
	elif ! grep -qxF "$line" "$2"; then
		echo "kernel config: wanted $line, got: $(grep "^$opt=" "$2" || echo unset)"
	fi
done)
[ -z "$bad" ] || { echo "$bad"; exit 1; }
