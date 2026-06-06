#!/bin/sh

set -eu

project_name=${COMPOSE_PROJECT_NAME:-ktk-schedule}
volume_name=${BACKUP_VOLUME_NAME:-${project_name}_ktk_schedule_data}
database_name=${BACKUP_DATABASE_NAME:-ktk-schedule.db}
backup_dir=${BACKUP_DIR:-backups}
keep_count=${BACKUP_KEEP_COUNT:-10}
keep_days=${BACKUP_KEEP_DAYS:-30}
verify_backup=${BACKUP_VERIFY:-false}
verify_db=

cleanup() {
	if [ -n "$verify_db" ]; then
		rm -f "$verify_db"
	fi
}

trap cleanup EXIT HUP INT TERM

mkdir -p "$backup_dir"

volume_mount=$(docker volume inspect "$volume_name" --format '{{.Mountpoint}}')
db_path="$volume_mount/$database_name"

if [ ! -f "$db_path" ]; then
	echo "backup skipped: database not found at $db_path"
	exit 0
fi

timestamp=$(date -u +%Y%m%d-%H%M%S)
backup_path="$backup_dir/$database_name-$timestamp.db"
compressed_path="$backup_path.gz"

if command -v sqlite3 >/dev/null 2>&1; then
	sqlite3 "$db_path" ".backup '$backup_path'"
else
	if [ "$verify_backup" = "true" ]; then
		echo "error: sqlite3 is required when BACKUP_VERIFY=true" >&2
		exit 1
	fi
	echo "warning: sqlite3 not found, falling back to file copy" >&2
	cp "$db_path" "$backup_path"
fi

gzip -f "$backup_path"
sha256sum "$compressed_path" >"$compressed_path.sha256"
gzip -t "$compressed_path"
sha256sum -c "$compressed_path.sha256"

if [ "$verify_backup" = "true" ]; then
	verify_db=$(mktemp "$backup_dir/.verify-$database_name.XXXXXX")
	gzip -cd "$compressed_path" >"$verify_db"
	integrity_result=$(sqlite3 "$verify_db" 'PRAGMA integrity_check;')
	if [ "$integrity_result" != "ok" ]; then
		echo "error: backup integrity check failed: $integrity_result" >&2
		exit 1
	fi
	echo "backup integrity check passed"
fi

find "$backup_dir" -name "$database_name-*.db.gz" -mtime +"$keep_days" -delete
find "$backup_dir" -name "$database_name-*.db.gz.sha256" -mtime +"$keep_days" -delete

find "$backup_dir" -name "$database_name-*.db.gz" -type f -printf '%T@ %p\n' |
	sort -rn |
	tail -n +"$((keep_count + 1))" |
	cut -d ' ' -f2- |
	while IFS= read -r old_backup; do
		rm -f "$old_backup" "$old_backup.sha256"
	done

echo "database backed up to $compressed_path"
