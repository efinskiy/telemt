use super::*;

pub(super) async fn handle(
    method: &Method,
    normalized_path: &str,
    query: Option<&str>,
    shared: &ApiShared,
    cfg: &ProxyConfig,
    config_rx: &watch::Receiver<Arc<ProxyConfig>>,
) -> Result<Option<Response<Full<Bytes>>>, ApiFailure> {
    let api_cfg = &cfg.server.api;
    match (method.as_str(), normalized_path) {
        ("GET", "/web-status") => Ok(web_status::render(query, &shared.web_trace).await),
        ("GET", "/v1/runtime/interfaces") => {
            let data = super::interface_inventory::collect(cfg, shared.active_runtime.load().id)?;
            Ok(success_response(StatusCode::OK, data, current_revision(&shared.config_path).await?))
        }
        ("GET", "/v1/fleet/compatibility") => {
            let mut credentials: Vec<_> = cfg.access.users.iter().collect();
            credentials.sort_unstable_by_key(|(user, _)| *user);
            let users: Vec<_> = credentials.iter().map(|(user, secret)| (*user, *secret, cfg.access.is_user_enabled(user))).collect();
            let mut listeners: Vec<_> = cfg.server.listeners.iter().map(|listener| serde_json::json!({"port": listener.port.unwrap_or(cfg.server.port), "transport": listener.transport}).to_string()).collect();
            listeners.sort_unstable(); listeners.dedup();
            // Public addresses differ by node; the port and WEB handshake contract must agree.
            let mut web_contract = serde_json::to_value(&cfg.web).map_err(|_| ApiFailure::bad_request("WEB configuration cannot be serialized"))?;
            if let Some(vhosts) = web_contract["vhosts"].as_array_mut() {
                for vhost in vhosts {
                    let port = vhost["public_addr"].as_str().and_then(|value| value.parse::<std::net::SocketAddr>().ok()).map(|address| address.port());
                    vhost["public_addr"] = serde_json::json!(port);
                    // Endpoint inventories differ across nodes without changing client capabilities.
                    if let Some(object) = vhost.as_object_mut() { object.remove("public_addrs"); }
                }
            }
            let manifest = serde_json::json!({
                "users": users,
                "web": web_contract,
                "tls_domain": cfg.censorship.tls_domain,
                "tls_domains": cfg.censorship.tls_domains,
                "listeners": listeners,
                "modes": cfg.general.modes,
                "port": cfg.server.port,
            });
            let fingerprint = hex::encode(crate::crypto::sha256(manifest.to_string().as_bytes()));
            let shared_users: Vec<_> = users.iter().filter(|(name, _, _)| *name != "fleet_probe").collect();
            let user_fingerprint = hex::encode(crate::crypto::sha256(serde_json::json!(shared_users).to_string().as_bytes()));
            let data = serde_json::json!({"fingerprint": fingerprint, "user_fingerprint": user_fingerprint, "shaping_supported": true});
            Ok(success_response(StatusCode::OK, data, current_revision(&shared.config_path).await?))
        }
        ("GET", "/v1/fleet/traffic") => {
            let mut up = 0u64;
            let mut down = 0u64;
            for entry in shared.stats.iter_user_stats() {
                up = up.saturating_add(entry.value().octets_from_client.load(Ordering::Relaxed));
                down = down.saturating_add(entry.value().octets_to_client.load(Ordering::Relaxed));
            }
            let shaping = shared.proxy_shared.traffic_limiter.shaping_snapshot(0, 0);
            if shaping.enabled { up = shaping.committed_bytes[0]; down = shaping.committed_bytes[1]; }
            // Reuse admission state so shared accounts and disabled telemetry do not distort IP counts.
            let clients = shared.ip_tracker.live_clients(10_000).await;
            let data = serde_json::json!({"octets_from_client": up, "octets_to_client": down,
                "active_ips": clients.total_ips,
                "active_ips_incomplete": clients.truncated || clients.cleanup_pending > 0,
                "available": shaping.enabled || shared.stats.telemetry_policy().user_enabled,
                "counter_source": if shaping.enabled { "shaping" } else { "user_stats" },
                "counter_instance": shaping.counter_instance,
                "runtime_generation": shared.active_runtime.load().id,
                "active_users": cfg.access.users.keys().filter(|user| shared.stats.get_process_user_curr_connects(user) > 0).count(),
                "telemetry_user_enabled": shared.stats.telemetry_policy().user_enabled,
                "current_connections": shared.stats.get_current_connections_total()});
            Ok(success_response(StatusCode::OK, data, current_revision(&shared.config_path).await?))
        }
        ("GET", "/v1/runtime/clients") => {
            let snapshot = shared.ip_tracker.live_clients(10_000).await;
            let data = serde_json::json!({"generated_at": chrono::Utc::now().to_rfc3339(), "snapshot": snapshot});
            Ok(success_response(StatusCode::OK, data, current_revision(&shared.config_path).await?))
        }
        ("GET", "/v1/runtime/shaping") => {
            let mut offset = 0usize;
            let mut limit = 100usize;
            if let Some(query) = query {
                for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
                    let parsed = value.parse::<usize>().map_err(|_| ApiFailure::bad_request("invalid shaping pagination"))?;
                    match key.as_ref() {
                        "offset" if parsed <= 131_072 => offset = parsed,
                        "limit" if (1..=1000).contains(&parsed) => limit = parsed,
                        _ => return Err(ApiFailure::bad_request("invalid shaping pagination")),
                    }
                }
            }
            let data = shared.proxy_shared.traffic_limiter.shaping_snapshot(offset, limit);
            Ok(success_response(StatusCode::OK, data, current_revision(&shared.config_path).await?))
        }
        ("GET", "/v1/health") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = HealthData {
                status: "ok",
                read_only: api_cfg.read_only,
            };
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/health/ready") => {
            let revision = current_revision(&shared.config_path).await?;
            let admission_open = shared.runtime_state.admission_open.load(Ordering::Relaxed);
            let upstream_health = shared.upstream_manager.api_health_summary().await;
            let ready = admission_open && upstream_health.healthy_total > 0;
            let reason = if ready {
                None
            } else if !admission_open {
                Some("admission_closed")
            } else {
                Some("no_healthy_upstreams")
            };
            let data = HealthReadyData {
                ready,
                status: if ready { "ready" } else { "not_ready" },
                reason,
                admission_open,
                healthy_upstreams: upstream_health.healthy_total,
                total_upstreams: upstream_health.configured_total,
            };
            let status_code = if ready {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            Ok(success_response(status_code, data, revision))
        }
        ("GET", "/v1/runtime/system-resources") => {
            let data = crate::api::system_resources::collect().await;
            Ok(success_response(StatusCode::OK, data, current_revision(&shared.config_path).await?))
        }
        ("GET", "/v1/system/info") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_system_info_data(shared, cfg, &revision);
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/runtime/gates") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_runtime_gates_data(shared, cfg).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/runtime/initialization") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_runtime_initialization_data(shared).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/limits/effective") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_limits_effective_data(cfg);
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/security/posture") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_security_posture_data(cfg);
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/security/whitelist") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_security_whitelist_data(cfg);
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/stats/summary") => {
            let revision = current_revision(&shared.config_path).await?;
            let connections_bad_by_class = shared
                .stats
                .get_connects_bad_class_counts()
                .into_iter()
                .map(|(class, total)| ClassCount { class, total })
                .collect();
            let handshake_failures_by_class = shared
                .stats
                .get_handshake_failure_class_counts()
                .into_iter()
                .map(|(class, total)| ClassCount { class, total })
                .collect();
            let data = SummaryData {
                uptime_seconds: shared.stats.uptime_secs(),
                connections_total: shared.stats.get_connects_all(),
                connections_bad_total: shared.stats.get_connects_bad(),
                connections_bad_by_class,
                handshake_failures_by_class,
                handshake_timeouts_total: shared.stats.get_handshake_timeouts(),
                configured_users: cfg.access.users.len(),
            };
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/stats/zero/all") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_zero_all_data(&shared.stats, cfg.access.users.len());
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/stats/upstreams") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_upstreams_data(shared, api_cfg);
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/stats/minimal/all") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_minimal_all_data(shared, api_cfg).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/stats/me-writers") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_me_writers_data(shared, api_cfg).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/stats/dcs") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_dcs_data(shared, api_cfg).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/runtime/me-pool-state") | ("GET", "/v1/runtime/me_pool_state") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_runtime_me_pool_state_data(shared).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/runtime/me-quality") | ("GET", "/v1/runtime/me_quality") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_runtime_me_quality_data(shared).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/runtime/upstream-quality") | ("GET", "/v1/runtime/upstream_quality") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_runtime_upstream_quality_data(shared).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/runtime/nat-stun") | ("GET", "/v1/runtime/nat_stun") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_runtime_nat_stun_data(shared).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/runtime/me-selftest") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_runtime_me_selftest_data(shared, cfg).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/runtime/connections/summary") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_runtime_connections_summary_data(shared, cfg).await;
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/runtime/events/recent") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_runtime_events_recent_data(shared, cfg, query);
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/runtime/tls-fingerprints") => {
            let revision = current_revision(&shared.config_path).await?;
            let data = build_runtime_tls_fingerprints_data(shared, cfg, query);
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/stats/users/active-ips") => {
            let revision = current_revision(&shared.config_path).await?;
            let usernames: Vec<_> = cfg.access.users.keys().cloned().collect();
            let active_ips_map = shared.ip_tracker.get_active_ips_for_users(&usernames).await;
            let mut data: Vec<UserActiveIps> = active_ips_map
                .into_iter()
                .filter(|(_, ips)| !ips.is_empty())
                .map(|(username, active_ips)| UserActiveIps {
                    username,
                    active_ips,
                })
                .collect();
            data.sort_by(|a, b| a.username.cmp(&b.username));
            Ok(success_response(StatusCode::OK, data, revision))
        }
        ("GET", "/v1/stats/users") | ("GET", "/v1/users") => {
            let revision = current_revision(&shared.config_path).await?;
            let disk_cfg = load_config_from_disk(&shared.config_path).await?;
            let runtime_cfg = config_rx.borrow().clone();
            let (detected_ip_v4, detected_ip_v6) = shared.detected_link_ips();
            let users = users_from_config(
                &disk_cfg,
                &shared.stats,
                &shared.ip_tracker,
                detected_ip_v4,
                detected_ip_v6,
                Some(runtime_cfg.as_ref()),
            )
            .await;
            Ok(success_response(StatusCode::OK, users, revision))
        }
        ("GET", "/v1/stats/users/quota") => {
            let revision = current_revision(&shared.config_path).await?;
            let disk_cfg = load_config_from_disk(&shared.config_path).await?;
            let data = build_user_quota_list(&disk_cfg, shared.stats.as_ref());
            Ok(success_response(StatusCode::OK, data, revision))
        }

        _ => return Ok(None),
    }
    .map(Some)
}
