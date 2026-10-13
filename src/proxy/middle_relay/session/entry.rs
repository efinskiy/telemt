use super::*;

/// Runs Middle-End relay with explicit kernel-conntrack close publication policy.
pub(crate) async fn handle_via_middle_proxy_with_conntrack<R, W>(
    crypto_reader: CryptoReader<R>,
    crypto_writer: CryptoWriter<W>,
    success: HandshakeSuccess,
    me_pool: Arc<MePool>,
    stats: Arc<Stats>,
    config: Arc<ProxyConfig>,
    buffer_pool: Arc<BufferPool>,
    local_addr: SocketAddr,
    rng: Arc<SecureRandom>,
    route_rx: watch::Receiver<RouteCutoverState>,
    route_snapshot: RouteCutoverState,
    session_id: u64,
    session_cancel: CancellationToken,
    shared: Arc<ProxySharedState>,
    conntrack_close_policy: ConntrackClosePolicy,
    quota_handle: UserQuotaHandle,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    handle_via_middle_proxy_with_shaping(crypto_reader, crypto_writer, success, me_pool, stats, config, buffer_pool, local_addr, rng, route_rx, route_snapshot, session_id, session_cancel, shared, conntrack_close_policy, quota_handle, None).await
}

