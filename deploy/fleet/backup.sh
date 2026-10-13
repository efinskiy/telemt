#!/bin/sh
set -eu
if [ "$#" -ne 1 ]; then echo "Usage: backup.sh OUTPUT.dump" >&2; exit 2; fi
umask 077
case "$1" in /*) output_path=$1 ;; *) output_path="$(pwd)/$1" ;; esac
cd "$(dirname "$0")"
docker compose -f compose.yml exec -T postgres pg_dump -U telemt -d telemt -Fc > "$output_path.part"
mv "$output_path.part" "$output_path"
echo "Database backup saved; preserve the master key separately."
