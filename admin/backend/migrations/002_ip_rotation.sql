-- Address-scoped results cannot inherit success from a node's previous public IP.
ALTER TABLE probe_jobs ADD COLUMN address text;
ALTER TABLE probe_jobs DROP CONSTRAINT probe_jobs_round_agent_id_node_id_family_key;
CREATE UNIQUE INDEX probe_job_endpoint ON probe_jobs(round,agent_id,node_id,family,coalesce(address,''));
CREATE TABLE ip_pools (
    id uuid PRIMARY KEY, node_id uuid NOT NULL UNIQUE REFERENCES nodes(id),
    group_id uuid NOT NULL UNIQUE REFERENCES failover_groups(id),
    owner_id uuid NOT NULL REFERENCES administrators(id),
    name text NOT NULL UNIQUE, addresses jsonb NOT NULL CHECK(jsonb_array_length(addresses) BETWEEN 2 AND 8),
    automatic boolean NOT NULL DEFAULT false, status text NOT NULL DEFAULT 'unprepared',
    preparation uuid REFERENCES operations(id) ON DELETE SET NULL, last_rotation timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE ip_rotations (
    id uuid PRIMARY KEY, pool_id uuid NOT NULL REFERENCES ip_pools(id), actor uuid,
    old_ip text NOT NULL, new_ip text NOT NULL, family integer NOT NULL CHECK(family IN(4,6)),
    state text NOT NULL CHECK(state IN('queued','dns_pending','retiring','succeeded','unknown','failed')),
    dns_change uuid REFERENCES dns_changes(id) ON DELETE SET NULL, retirement uuid REFERENCES operations(id) ON DELETE SET NULL,
    reason text NOT NULL, result jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX one_pending_rotation ON ip_rotations(pool_id) WHERE state IN('queued','dns_pending','retiring','unknown');
CREATE TABLE address_health (
    node_id uuid NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, address text NOT NULL,
    status text NOT NULL DEFAULT 'unknown', PRIMARY KEY(node_id,address)
);
CREATE TRIGGER ip_pools_bound BEFORE INSERT ON ip_pools FOR EACH ROW EXECUTE FUNCTION enforce_fleet_bound();
