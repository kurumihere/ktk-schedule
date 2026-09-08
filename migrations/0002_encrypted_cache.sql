-- Rebuild the disposable cache with authenticated encryption on next use.
-- Accounts, passwords and notification settings remain intact.
DELETE FROM schedules;
DELETE FROM views;
