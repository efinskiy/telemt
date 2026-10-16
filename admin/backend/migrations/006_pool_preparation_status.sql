-- Preparation diagnostics are independent of DNS admission and never contain agent secrets.
ALTER TABLE ip_pools ADD COLUMN preparation_error text;
ALTER TABLE ip_pools ADD COLUMN preparation_checked_at timestamptz;
