-- Retain legacy identities for explicit owner cleanup; never contact their old endpoints again.
UPDATE failover_groups SET automatic=false,status='registration_required',
    document=jsonb_set(document,'{configuration_verified}','false')
WHERE active_node IN (SELECT id FROM nodes WHERE NOT EXISTS(SELECT 1 FROM managed_nodes WHERE node_id=nodes.id))
   OR EXISTS(SELECT 1 FROM nodes n WHERE failover_groups.document->'node_ids' @> jsonb_build_array(n.id)
             AND NOT EXISTS(SELECT 1 FROM managed_nodes m WHERE m.node_id=n.id));
UPDATE profiles SET resume_automatic=false WHERE group_id IN
    (SELECT id FROM failover_groups WHERE status='registration_required');
UPDATE ip_pools SET automatic=false,status='registration_required'
WHERE NOT EXISTS(SELECT 1 FROM managed_nodes WHERE node_id=ip_pools.node_id);
UPDATE nodes SET enabled=false,api_url='retired://unsupported',token_cipher='',probe_cipher='',
    collected_at=NULL,snapshot='{"management":"registration_required","ready":false}'
WHERE NOT EXISTS(SELECT 1 FROM managed_nodes WHERE node_id=nodes.id);
