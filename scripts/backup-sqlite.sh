#!/bin/sh

set -eu

project_name=${COMPOSE_PROJECT_NAME:-ktk-schedule}
volume_name=${BACKUP_VOLUME_NAME:-${project_name}_ktk_schedule_data}
database_name=${BACKUP_DATABASE_NAME:-ktk-schedule.db}
backup_dir=${BACKUP_DIR:-backups}
keep_count=${BACKUP_KEEP_COUNT:-10}
keep_days=${BACKUP_KEEP_DAYS:-30}
verify_backup=${BACKUP_VERIFY:-false}
helper_image=${BACKUP_HELPER_IMAGE:-${KTK_SCHEDULE_IMAGE:-}}
force_helper=${BACKUP_FORCE_HELPER:-false}
verify_db=

cleanup() {
	if [ -n "$verify_db" ]; then
		rm -f "$verify_db"
	fi
}

trap cleanup EXIT HUP INT TERM

mkdir -p "$backup_dir"
backup_dir_abs=$(cd "$backup_dir" && pwd)

volume_mount=$(docker volume inspect "$volume_name" --format '{{.Mountpoint}}')
db_path="$volume_mount/$database_name"

timestamp=$(date -u +%Y%m%d-%H%M%S)
backup_path="$backup_dir/$database_name-$timestamp.db"
compressed_path="$backup_path.gz"
backup_file="$database_name-$timestamp.db"

prune_backups() {
	find "$backup_dir" -name "$database_name-*.db.gz" -mtime +"$keep_days" -delete
	find "$backup_dir" -name "$database_name-*.db.gz.sha256" -mtime +"$keep_days" -delete

	find "$backup_dir" -name "$database_name-*.db.gz" -type f -printf '%T@ %p\n' |
		sort -rn |
		tail -n +"$((keep_count + 1))" |
		cut -d ' ' -f2- |
		while IFS= read -r old_backup; do
			rm -f "$old_backup" "$old_backup.sha256"
		done
}

if { [ "$force_helper" = "true" ] || [ ! -f "$db_path" ]; } && [ -n "$helper_image" ]; then
	docker run --rm \
		--user "$(id -u):$(id -g)" \
		-v "$volume_name:/data:ro" \
		-v "$backup_dir_abs:/backup" \
		-e BACKUP_DATABASE_NAME="$database_name" \
		-e BACKUP_FILE="$backup_file" \
		-e BACKUP_VERIFY="$verify_backup" \
		--entrypoint /bin/sh \
		"$helper_image" -eu -c '
db_path="/data/$BACKUP_DATABASE_NAME"
backup_path="/backup/$BACKUP_FILE"
compressed_path="$backup_path.gz"
verify_db=

cleanup() {
	if [ -n "$verify_db" ]; then
		rm -f "$verify_db"
	fi
}
trap cleanup EXIT HUP INT TERM

if [ ! -f "$db_path" ]; then
	echo "backup skipped: database not found at $db_path"
	exit 0
fi
if ! command -v sqlite3 >/dev/null 2>&1; then
	echo "error: sqlite3 is required in BACKUP_HELPER_IMAGE" >&2
	exit 1
fi

sqlite3 "file:$db_path?mode=ro" ".backup '\''$backup_path'\''"
gzip -f "$backup_path"
sha256sum "$compressed_path" >"$compressed_path.sha256"
gzip -t "$compressed_path"
sha256sum -c "$compressed_path.sha256"

if [ "$BACKUP_VERIFY" = "true" ]; then
	verify_db=$(mktemp "/backup/.verify-$BACKUP_DATABASE_NAME.XXXXXX")
	gzip -cd "$compressed_path" >"$verify_db"
	integrity_result=$(sqlite3 "$verify_db" "PRAGMA integrity_check;")
	if [ "$integrity_result" != "ok" ]; then
		echo "error: backup integrity check failed: $integrity_result" >&2
		exit 1
	fi
	echo "backup integrity check passed"
fi
'

	if [ -f "$compressed_path" ]; then
		prune_backups
		echo "database backed up to $compressed_path"
	fi
	exit 0
fi

if [ ! -f "$db_path" ]; then
	echo "backup skipped: database not found at $db_path"
	exit 0
fi

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

prune_backups
echo "database backed up to $compressed_path"
