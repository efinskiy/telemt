#!/bin/sh
set -eu
if [ "$#" -ne 1 ]; then echo "Usage: restore.sh BACKUP.dump (replaces the stopped cabinet database)" >&2; exit 2; fi
case "$1" in /*) backup_path=$1 ;; *) backup_path="$(pwd)/$1" ;; esac
cd "$(dirname "$0")"
docker compose -f compose.yml stop admin nginx
docker compose -f compose.yml exec -T postgres pg_restore -U telemt -d telemt --clean --if-exists --single-transaction < "$backup_path"
docker compose -f compose.yml up -d admin nginx
echo "Restored database; master key must match the backup. Review unknown operations before enabling DNS automation."
