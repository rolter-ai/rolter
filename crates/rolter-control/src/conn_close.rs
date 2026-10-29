//! Closing one connection from outside its request (#3477).
//!
//! A response body can only end a connection when hyper polls it, and hyper
//! stops polling a body while its socket buffer is full, which is exactly what
//! a client that stops reading causes. So the listener hands every accepted
//! socket a [`CloseSwitch`] and wraps it so that a tripped switch fails the
//! next read or write (and wakes a task parked on one); hyper then drops the
//! connection and the socket closes. A handler finds the switch of its own
//! connection by the peer address that connect info already carries.
//!
//! This is per connection and opt-in: only a handler that asks for its switch
//! can close anything, so slow but healthy streams elsewhere are untouched.

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    task::{Context, Poll, Waker},
};

use parking_lot::Mutex;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
};

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
}

type Registry = Mutex<HashMap<SocketAddr, Arc<CloseSwitch>>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Registry::default)
}

/// The switch of the live connection whose peer is `peer`, if the listener
/// that accepted it was a [`CloseableListener`].
pub fn switch_for(peer: SocketAddr) -> Option<Arc<CloseSwitch>> {
    registry().lock().get(&peer).cloned()
}

/// A socket that fails once its [`CloseSwitch`] is tripped.
pub struct Closeable {
    inner: TcpStream,
    switch: Arc<CloseSwitch>,
    peer: SocketAddr,
}

impl Drop for Closeable {
    fn drop(&mut self) {
        let mut live = registry().lock();
        // a reused peer port must not unregister its newer connection
        if live
            .get(&self.peer)
            .is_some_and(|switch| Arc::ptr_eq(switch, &self.switch))
        {
            live.remove(&self.peer);
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
        self.switch.wakers.lock().0 = Some(cx.waker().clone());
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
        self.switch.wakers.lock().1 = Some(cx.waker().clone());
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
        registry().lock().insert(peer, Arc::clone(&switch));
        (
            Closeable {
                inner,
                switch,
                peer,
            },
            peer,
        )
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

/// The make-service for a [`CloseableListener`]: the app with the peer
/// address as connect info, as `into_make_service_with_connect_info` does for a
/// plain `TcpListener` (axum implements `Connected` only for that listener).
#[derive(Clone)]
pub struct MakeCloseable(pub axum::Router);

impl<'a> tower::Service<axum::serve::IncomingStream<'a, CloseableListener>> for MakeCloseable {
    type Response = axum::Router;
    type Error = std::convert::Infallible;
    type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, stream: axum::serve::IncomingStream<'a, CloseableListener>) -> Self::Future {
        let peer = *stream.remote_addr();
        std::future::ready(Ok(self
            .0
            .clone()
            .layer(axum::Extension(axum::extract::ConnectInfo(peer)))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn a_tripped_switch_fails_a_write_parked_on_a_full_socket() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut listener = CloseableListener(listener);
        // a client that connects and never reads
        let _client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (mut io, peer) = axum::serve::Listener::accept(&mut listener).await;
        let switch = switch_for(peer).expect("the connection registered its switch");
        let writer = tokio::spawn(async move {
            let chunk = vec![0u8; 64 * 1024];
            loop {
                io.write_all(&chunk).await?;
            }
            #[allow(unreachable_code)]
            Ok::<(), io::Error>(())
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!writer.is_finished(), "the socket buffer never filled");
        switch.trip();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), writer)
            .await
            .expect("the parked write woke")
            .unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(
            switch_for(peer).is_none(),
            "the dropped socket unregistered"
        );
    }

    #[tokio::test]
    async fn an_untripped_connection_carries_traffic() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut listener = CloseableListener(listener);
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (mut io, _) = axum::serve::Listener::accept(&mut listener).await;
        io.write_all(b"ping").await.unwrap();
        let mut got = [0u8; 4];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping");
    }
}
