use super::*;
use std::{io, pin::Pin, task::{Context, Poll}};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};

/// Paces authenticated transport I/O without buffering or changing protocol order.
pub(crate) struct AdaptiveIo<S> {
    inner: S,
    lease: Option<Arc<TrafficLease>>,
    waits: [Option<Pin<Box<Sleep>>>; 2],
    armed: [bool; 2],
    wait_started: [Option<Instant>; 2],
    activity: Option<(Arc<AtomicU64>, Instant)>,
}

impl<S> AdaptiveIo<S> {
    /// Uses one logical flow for both directions and all transport wrappers.
    pub(crate) fn new(inner: S, lease: Option<Arc<TrafficLease>>) -> Self {
        Self { inner, lease, waits: [None, None], armed: [false; 2], wait_started: [None, None], activity: None }
    }
    /// Removes pacing when an authenticated handshake falls back to raw transport.
    pub(crate) fn into_inner(self) -> S { self.inner }
    /// Shares Middle relay's existing activity epoch during intentional pacing.
    pub(crate) fn with_activity(mut self, activity: Arc<AtomicU64>, epoch: Instant) -> Self {
        self.activity = Some((activity, epoch)); self
    }
    // Avoid constructing a reservation-shaped return value in the disabled forwarding path.
    #[inline]
    fn bypass(&mut self) -> bool {
        if self.lease.as_ref().is_none_or(|lease| !lease.limiter.adaptive.enabled()) {
            if self.armed[0] || self.armed[1] { self.armed = [false; 2]; self.wait_started = [None, None]; }
            true
        } else { false }
    }
    fn arm(&mut self, cx: &mut Context<'_>, i: usize) {
        let deadline = Instant::now() + next_refill_delay();
        if let Some(wait) = self.waits[i].as_mut() { wait.as_mut().reset(deadline); }
        else { self.waits[i] = Some(Box::pin(tokio::time::sleep_until(deadline))); }
        self.armed[i] = true;
        self.wait_started[i] = Some(Instant::now());
        if let Some(wait) = self.waits[i].as_mut() { let _ = wait.as_mut().poll(cx); }
    }
    fn reserve(&mut self, cx: &mut Context<'_>, direction: RateDirection, bytes: usize) -> Poll<io::Result<Option<AdaptiveReservation>>> {
        let Some(lease) = self.lease.as_ref() else { return Poll::Ready(Ok(None)); };
        if !lease.limiter.adaptive.enabled() {
            self.armed = [false; 2]; self.wait_started = [None, None];
            return Poll::Ready(Ok(None));
        }
        let i = index(direction);
        if self.armed[i] {
            if let Some(wait) = self.waits[i].as_mut() {
                if wait.as_mut().poll(cx).is_pending() { return Poll::Pending; }
            }
            self.armed[i] = false;
            if let Some(started) = self.wait_started[i].take() {
                if let Some(flow) = lease.adaptive.load_full() {
                    let elapsed = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                    flow.wait_ms[i].fetch_add(elapsed, Ordering::Relaxed);
                    flow.owner.waits_ms.fetch_add(elapsed, Ordering::Relaxed);
                }
            }
        }
        let Some(flow) = lease.adaptive_flow() else {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::ConnectionAborted, "adaptive IP admission exhausted")));
        };
        let reservation = flow.reserve(direction, bytes as u64);
        if reservation.granted > 0 || bytes == 0 { return Poll::Ready(Ok(Some(reservation))); }
        if let Some(activity) = lease.shaping_activity.load().as_ref() { activity.counter.store(activity.epoch.elapsed().as_millis() as u64, Ordering::Relaxed); }
        if let Some((activity, epoch)) = &self.activity { activity.store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed); }

        self.arm(cx, i);
        Poll::Pending
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for AdaptiveIo<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.bypass() { return Pin::new(&mut this.inner).poll_read(cx, buf); }
        let mut reservation = match this.reserve(cx, RateDirection::Up, buf.remaining()) { Poll::Pending => return Poll::Pending, Poll::Ready(Ok(r)) => r, Poll::Ready(Err(error)) => return Poll::Ready(Err(error)) };
        let Some(grant) = reservation.as_mut() else { return Pin::new(&mut this.inner).poll_read(cx, buf); };
        if buf.remaining() == 0 { return Poll::Ready(Ok(())); }
        let size = (grant.granted as usize).min(buf.remaining());
        let mut limited = ReadBuf::new(buf.initialize_unfilled_to(size));
        match Pin::new(&mut this.inner).poll_read(cx, &mut limited) {
            Poll::Ready(Ok(())) => { let n = limited.filled().len(); buf.advance(n); grant.settle(n as u64); Poll::Ready(Ok(())) }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)), Poll::Pending => { grant.idle_read(); Poll::Pending },
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for AdaptiveIo<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.bypass() { return Pin::new(&mut this.inner).poll_write(cx, bytes); }
        let mut reservation = match this.reserve(cx, RateDirection::Down, bytes.len()) { Poll::Pending => return Poll::Pending, Poll::Ready(Ok(r)) => r, Poll::Ready(Err(error)) => return Poll::Ready(Err(error)) };
        let size = reservation.as_ref().map(|r| r.granted as usize).unwrap_or(bytes.len()).min(bytes.len());
        match Pin::new(&mut this.inner).poll_write(cx, &bytes[..size]) {
            Poll::Ready(Ok(n)) => { if let Some(r) = reservation.as_mut() { r.settle(n as u64); } Poll::Ready(Ok(n)) }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)), Poll::Pending => { if let Some(reservation) = reservation.as_ref() { reservation.idle_read(); } Poll::Pending },
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> { Pin::new(&mut self.get_mut().inner).poll_flush(cx) }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> { Pin::new(&mut self.get_mut().inner).poll_shutdown(cx) }
}
