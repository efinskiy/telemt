use super::{FakeTlsReader, FakeTlsWriter};

impl<R> FakeTlsReader<R> {
    /// Replaces the transport without dropping a pending TLS header or record body.
    pub(crate) fn map_inner<T>(self, map: impl FnOnce(R) -> T) -> FakeTlsReader<T> {
        FakeTlsReader {
            upstream: map(self.upstream),
            state: self.state,
            body_scratch: self.body_scratch,
        }
    }
}
impl<W> FakeTlsWriter<W> {
    /// Replaces the transport while preserving any partially written TLS record.
    pub(crate) fn map_inner<T>(self, map: impl FnOnce(W) -> T) -> FakeTlsWriter<T> {
        FakeTlsWriter {
            upstream: map(self.upstream),
            state: self.state,
            record_buffer: self.record_buffer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::traffic_limiter::{AdaptiveIo, TrafficLimiter};
    use std::{
        io,
        pin::Pin,
        sync::Arc,
        sync::atomic::{AtomicU64, Ordering},
        task::{Context, Poll},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct PartialWriter(Arc<AtomicU64>);
    impl tokio::io::AsyncWrite for PartialWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            let n = bytes.len().min(3);
            self.0.fetch_add(n as u64, Ordering::Relaxed);
            Poll::Ready(Ok(n))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    #[tokio::test]
    async fn tls_buffer_acceptance_and_cancelled_flush_count_only_transport_writes() {
        let limiter = TrafficLimiter::new();
        limiter.apply_shaping_from_source(
            1,
            crate::config::ShapingConfig {
                enabled: true,
                up_bps: 800,
                down_bps: 800,
                ..Default::default()
            },
        );
        let lease = limiter
            .acquire_lease("test", "192.0.2.1".parse().unwrap())
            .unwrap();
        let physical = Arc::new(AtomicU64::new(0));
        let mut writer = FakeTlsWriter::new(PartialWriter(physical.clone()))
            .map_inner(|inner| AdaptiveIo::new(inner, Some(lease.clone())));
        writer.write_all(&[9; 128]).await.unwrap();
        assert!(
            physical.load(Ordering::Relaxed) < 128,
            "buffer admission must not be counted as a TCP write"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), writer.flush())
                .await
                .is_err()
        );
        let committed = physical.load(Ordering::Relaxed);
        assert_eq!(limiter.shaping_snapshot(0, 1).committed_bytes[1], committed);
        assert!(
            committed <= 4,
            "cancelled flush must preserve the global epoch budget"
        );
        limiter.apply_shaping_from_source(2, Default::default());
        writer.flush().await.unwrap();
        assert_eq!(physical.load(Ordering::Relaxed), 133);
        assert_eq!(limiter.shaping_snapshot(0, 1).committed_bytes[1], committed);
    }
    #[tokio::test]
    async fn transport_replacement_keeps_already_buffered_tls_payload() {
        let (mut sender, transport) = tokio::io::duplex(64);
        sender
            .write_all(&[0x17, 3, 3, 0, 4, 1, 2, 3, 4])
            .await
            .unwrap();
        let mut reader = FakeTlsReader::new(transport);
        let first = reader.read_exact(1).await.unwrap();
        assert_eq!(first.as_ref(), &[1]);
        let mut mapped = reader.map_inner(|inner| AdaptiveIo::new(inner, None));
        let mut rest = [0; 3];
        AsyncReadExt::read_exact(&mut mapped, &mut rest)
            .await
            .unwrap();
        assert_eq!(rest, [2, 3, 4]);
    }
}
