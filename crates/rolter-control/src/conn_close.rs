//! Closing one connection from outside its request (#3477).
//!
//! A response body can only end a connection when hyper polls it, and hyper
//! stops polling a body while its socket buffer is full, which is exactly what
//! a client that stops reading causes. So the listener hands every accepted
//! socket a [`CloseSwitch`] and wraps it so that a tripped switch fails the
//! next read or write (and wakes a task parked on one); hyper then drops the
//! connection and the socket closes, with a reset so that data still queued for a
//! peer that stopped reading is discarded. A handler takes the switch of its own
//! connection from the request as a [`ConnectionHandle`].
//!
//! This is per connection and opt-in: only a handler that asks for its switch
//! can close anything, so slow but healthy streams elsewhere are untouched. A
//! connection is one client only on HTTP/1.x; on HTTP/2 it carries many
//! requests, so a handler must not trip it there.

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll, Waker},
};

use parking_lot::Mutex;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
};
use tower::Layer as _;

/// Closes one connection when tripped.
#[derive(Debug, Default)]
pub struct CloseSwitch {
    tripped: AtomicBool,
    wakers: Mutex<(Option<Waker>, Option<Waker>)>,
}

impl CloseSwitch {
    /// Fail the connection's next read or write, waking one parked on either.
    pub fn trip(&self) {
        self.tripped.store(true, Ordering::Release);
        let (read, write) = &mut *self.wakers.lock();
        for waker in [read.take(), write.take()].into_iter().flatten() {
            waker.wake();
        }
    }

    /// The switch has been tripped.
    pub fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::Acquire)
    }

    fn closed() -> io::Error {
        io::Error::new(io::ErrorKind::TimedOut, "connection closed by the server")
    }

    /// Remember `waker` in `slot`, cloning only when it is not the one held.
    fn park(slot: &mut Option<Waker>, waker: &Waker) {
        if !slot.as_ref().is_some_and(|held| held.will_wake(waker)) {
            *slot = Some(waker.clone());
        }
    }
}

/// The handle a handler takes from the request to close its own connection.
#[derive(Clone, Debug)]
pub struct ConnectionHandle(pub Arc<CloseSwitch>);

/// A socket that fails once its [`CloseSwitch`] is tripped.
pub struct Closeable {
    inner: TcpStream,
    switch: Arc<CloseSwitch>,
}

impl Drop for Closeable {
    fn drop(&mut self) {
        // a socket closed with unsent data still queued would otherwise linger
        // in FIN-WAIT, probing the stalled peer's zero window for minutes, which
        // is the hold the switch exists to end. an RST discards the queue
        if self.switch.is_tripped() {
            let _ = self.inner.set_zero_linger();
        }
    }
}

impl AsyncRead for Closeable {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.switch.is_tripped() {
            return Poll::Ready(Err(CloseSwitch::closed()));
        }
        CloseSwitch::park(&mut self.switch.wakers.lock().0, cx.waker());
        // a trip between the check and the registration would otherwise be lost
        if self.switch.is_tripped() {
            return Poll::Ready(Err(CloseSwitch::closed()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Closeable {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.arm(cx) {
            return Poll::Ready(Err(CloseSwitch::closed()));
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        if self.arm(cx) {
            return Poll::Ready(Err(CloseSwitch::closed()));
        }
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.arm(cx) {
            return Poll::Ready(Err(CloseSwitch::closed()));
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl Closeable {
    /// Register the writer's waker, then report whether the switch is tripped.
    fn arm(&self, cx: &Context<'_>) -> bool {
        if self.switch.is_tripped() {
            return true;
        }
        CloseSwitch::park(&mut self.switch.wakers.lock().1, cx.waker());
        self.switch.is_tripped()
    }
}

/// A [`TcpListener`] whose connections can be closed from a handler.
pub struct CloseableListener(pub TcpListener);

impl axum::serve::Listener for CloseableListener {
    type Io = Closeable;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (inner, peer) = axum::serve::Listener::accept(&mut self.0).await;
        let switch = Arc::new(CloseSwitch::default());
        (Closeable { inner, switch }, peer)
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

/// The make-service for a [`CloseableListener`]: the app with the peer address
/// as connect info, as `into_make_service_with_connect_info` does for a plain
/// `TcpListener` (axum implements `Connected` only for that listener), and the
/// connection's [`ConnectionHandle`].
///
/// Axum awaits `call` in the accept loop, so it only wraps the router once per
/// connection: `Router::layer` would rebuild every route there.
#[derive(Clone)]
pub struct MakeCloseable(pub axum::Router);

type PerConnection = <axum::Extension<ConnectionHandle> as tower::Layer<
    <axum::Extension<axum::extract::ConnectInfo<SocketAddr>> as tower::Layer<axum::Router>>::Service,
>>::Service;

impl<'a> tower::Service<axum::serve::IncomingStream<'a, CloseableListener>> for MakeCloseable {
    type Response = PerConnection;
    type Error = std::convert::Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, stream: axum::serve::IncomingStream<'a, CloseableListener>) -> Self::Future {
        let handle = ConnectionHandle(Arc::clone(&stream.io().switch));
        let peer = *stream.remote_addr();
        let with_peer = axum::Extension(axum::extract::ConnectInfo(peer)).layer(self.0.clone());
        std::future::ready(Ok(axum::Extension(handle).layer(with_peer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn accepted() -> (tokio::net::TcpStream, Closeable) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut listener = CloseableListener(listener);
        let client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (io, _) = axum::serve::Listener::accept(&mut listener).await;
        (client, io)
    }

    #[tokio::test]
    async fn a_tripped_switch_fails_a_write_parked_on_a_full_socket() {
        // a client that connects and never reads
        let (_client, mut io) = accepted().await;
        let switch = Arc::clone(&io.switch);
        let mut writer = tokio::spawn(async move {
            let chunk = vec![0u8; 64 * 1024];
            loop {
                io.write_all(&chunk).await?;
            }
            #[allow(unreachable_code)]
            Ok::<(), io::Error>(())
        });
        // parked: the socket buffers filled and the write is waiting
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(300), &mut writer)
                .await
                .is_err(),
            "the socket buffer never filled"
        );
        switch.trip();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), writer)
            .await
            .expect("the parked write woke")
            .unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn a_closed_stalled_socket_resets_the_peer_and_drops_its_queue() {
        let (mut client, mut io) = accepted().await;
        let switch = Arc::clone(&io.switch);
        let writer = tokio::spawn(async move {
            let chunk = vec![0u8; 64 * 1024];
            while io.write_all(&chunk).await.is_ok() {}
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        switch.trip();
        writer.await.unwrap();
        // the peer never read, and learns of the close at once: what it holds
        // is only what its buffers took before the stall, then a reset. a
        // close that left the queue to drain would deliver megabytes first
        let mut total = 0usize;
        let mut sink = vec![0u8; 64 * 1024];
        let started = std::time::Instant::now();
        loop {
            match client.read(&mut sink).await {
                Ok(0) | Err(_) => break,
                Ok(n) => total += n,
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "the peer was still being sent data ({total} bytes)"
            );
        }
    }

    #[tokio::test]
    async fn an_untripped_connection_carries_traffic() {
        let (mut client, mut io) = accepted().await;
        io.write_all(b"ping").await.unwrap();
        let mut got = [0u8; 4];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping");
    }
}
