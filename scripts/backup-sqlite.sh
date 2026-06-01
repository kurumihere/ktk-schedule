#!/bin/sh

set -eu

project_name=${COMPOSE_PROJECT_NAME:-ktk-schedule}
volume_name=${BACKUP_VOLUME_NAME:-${project_name}_ktk_schedule_data}
database_name=${BACKUP_DATABASE_NAME:-ktk-schedule.db}
backup_dir=${BACKUP_DIR:-backups}
keep_count=${BACKUP_KEEP_COUNT:-10}
keep_days=${BACKUP_KEEP_DAYS:-30}

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
    echo "warning: sqlite3 not found, falling back to file copy" >&2
    cp "$db_path" "$backup_path"
fi

gzip -f "$backup_path"
sha256sum "$compressed_path" > "$compressed_path.sha256"

find "$backup_dir" -name "$database_name-*.db.gz" -mtime +"$keep_days" -delete
find "$backup_dir" -name "$database_name-*.db.gz.sha256" -mtime +"$keep_days" -delete

ls -t "$backup_dir"/"$database_name"-*.db.gz 2>/dev/null | tail -n +"$((keep_count + 1))" | while IFS= read -r old_backup; do
    rm -f "$old_backup" "$old_backup.sha256"
done

echo "database backed up to $compressed_path"
