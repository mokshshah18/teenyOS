#!/bin/sh
# Fail if any option requested in the fragment ($1) did not survive into the
# final kernel .config ($2), e.g. because a dependency was missing.
set -eu
bad=0
grep -E '^CONFIG_[A-Z0-9_]+=' "$1" | while IFS= read -r line; do
	opt=${line%%=*}
	want=${line#*=}
	if [ "$want" = n ]; then
		if grep -q "^$opt=y" "$2"; then echo "kernel config: $opt is still enabled"; exit 1; fi
	elif ! grep -qxF "$line" "$2"; then
		echo "kernel config: wanted $line, got: $(grep "^$opt=" "$2" || echo unset)"
		exit 1
	fi
done || bad=1
exit $bad
