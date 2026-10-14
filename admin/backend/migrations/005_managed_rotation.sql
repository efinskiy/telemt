-- Each managed server owns an endpoint pool within the same DNS group.
ALTER TABLE ip_pools DROP CONSTRAINT ip_pools_group_id_key;
ALTER TABLE ip_pools ADD COLUMN deployment bigint REFERENCES deployments(id) ON DELETE SET NULL;
-- Old listener operations cannot be replayed by the managed-only controller.
UPDATE ip_pools SET automatic=false,status='unprepared',preparation=NULL;
UPDATE ip_rotations SET state='failed',result='{"reason":"managed_rotation_upgrade_requires_reconciliation"}' WHERE state IN ('queued','retiring');
DELETE FROM probe_jobs;
DELETE FROM address_health;
DELETE FROM node_health;
-- Force endpoint-aware bundles before readiness can re-admit upgraded nodes.
UPDATE managed_nodes SET desired_revision=0,status='pending';

UPDATE failover_groups g SET document=jsonb_set(g.document,'{profile_id}',to_jsonb(m.profile_id::text)) FROM managed_nodes m WHERE m.node_id=g.active_node;

UPDATE ip_rotations r SET state='succeeded',result='{"reason":"managed_upgrade_keeps_prepared_listeners","dns_verified":true}' FROM dns_changes d WHERE r.dns_change=d.id AND r.state='unknown' AND d.state='verified';
UPDATE ip_rotations r SET state='dns_pending' FROM dns_changes d WHERE r.dns_change=d.id AND r.state='unknown' AND d.state IN ('writing','unknown');
UPDATE ip_rotations SET state='failed',result='{"reason":"retired_control_api_operation"}' WHERE state='unknown' AND dns_change IS NULL;

ALTER TABLE ip_pools DROP COLUMN preparation;
ALTER TABLE ip_rotations DROP COLUMN retirement;
ALTER TABLE probe_jobs DROP COLUMN web_required;
ALTER TABLE probe_jobs ALTER COLUMN address SET NOT NULL;
ALTER TABLE nodes DROP COLUMN api_url;
ALTER TABLE ip_rotations DROP CONSTRAINT ip_rotations_state_check;
ALTER TABLE ip_rotations ADD CONSTRAINT ip_rotations_state_check CHECK(state IN ('queued','dns_pending','succeeded','unknown','failed'));
DROP INDEX one_pending_rotation;
CREATE UNIQUE INDEX one_pending_rotation ON ip_rotations(pool_id) WHERE state IN ('queued','dns_pending','unknown');
