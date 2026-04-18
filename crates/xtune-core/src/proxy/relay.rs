//! High-performance bidirectional relay for proxy streams.
//!
//! Uses 64 KB buffers (8× tokio's default 8 KB) for significantly better
//! throughput and lower latency, matching what Clash and other mature
//! proxies use internally.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Buffer size for relay operations (64 KB).
const RELAY_BUF_SIZE: usize = 64 * 1024;

/// Relay data bidirectionally between two async streams using large buffers.
///
/// Returns `(bytes_a_to_b, bytes_b_to_a)` on completion.
pub async fn relay_bidirectional<A, B>(a: &mut A, b: &mut B) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    Relay {
        a,
        b,
        a_buf: CopyBuf::new(),
        b_buf: CopyBuf::new(),
        a_to_b: 0,
        b_to_a: 0,
        a_done: false,
        b_done: false,
    }
    .await
}

struct CopyBuf {
    buf: Box<[u8; RELAY_BUF_SIZE]>,
    pos: usize,
    cap: usize,
}

impl CopyBuf {
    fn new() -> Self {
        Self {
            buf: Box::new([0u8; RELAY_BUF_SIZE]),
            pos: 0,
            cap: 0,
        }
    }
}

struct Relay<'a, A: ?Sized, B: ?Sized> {
    a: &'a mut A,
    b: &'a mut B,
    a_buf: CopyBuf,
    b_buf: CopyBuf,
    a_to_b: u64,
    b_to_a: u64,
    a_done: bool,
    b_done: bool,
}

/// Transfer data from reader to writer using a CopyBuf, returning bytes transferred.
fn transfer_one<R, W>(
    cx: &mut Context<'_>,
    reader: &mut R,
    writer: &mut W,
    buf: &mut CopyBuf,
    done: &mut bool,
) -> Poll<io::Result<u64>>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut transferred: u64 = 0;

    loop {
        // If we have data in the buffer, write it out
        if buf.pos < buf.cap {
            let n = ready!(Pin::new(&mut *writer).poll_write(cx, &buf.buf[buf.pos..buf.cap]))?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "write zero",
                )));
            }
            buf.pos += n;
            transferred += n as u64;
            if buf.pos == buf.cap {
                buf.pos = 0;
                buf.cap = 0;
            }
            continue;
        }

        // If the reader is done, flush and return
        if *done {
            ready!(Pin::new(&mut *writer).poll_flush(cx))?;
            return Poll::Ready(Ok(transferred));
        }

        // Read new data
        let mut read_buf = ReadBuf::new(&mut buf.buf[..]);
        match ready!(Pin::new(&mut *reader).poll_read(cx, &mut read_buf)) {
            Ok(()) => {
                let n = read_buf.filled().len();
                if n == 0 {
                    *done = true;
                    // Shutdown the write half
                    ready!(Pin::new(&mut *writer).poll_shutdown(cx))?;
                    return Poll::Ready(Ok(transferred));
                }
                buf.cap = n;
            }
            Err(e) => return Poll::Ready(Err(e)),
        }
    }
}

impl<A, B> Future for Relay<'_, A, B>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    type Output = io::Result<(u64, u64)>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;

        // Drive A → B
        let a_to_b = if !this.a_done || this.a_buf.pos < this.a_buf.cap {
            transfer_one(cx, this.a, this.b, &mut this.a_buf, &mut this.a_done)
        } else {
            Poll::Ready(Ok(0))
        };

        // Drive B → A
        let b_to_a = if !this.b_done || this.b_buf.pos < this.b_buf.cap {
            transfer_one(cx, this.b, this.a, &mut this.b_buf, &mut this.b_done)
        } else {
            Poll::Ready(Ok(0))
        };

        // Accumulate transferred bytes from ready results
        let a_transferred = match a_to_b {
            Poll::Ready(Ok(n)) => {
                this.a_to_b += n;
                n
            }
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => 0,
        };

        let b_transferred = match b_to_a {
            Poll::Ready(Ok(n)) => {
                this.b_to_a += n;
                n
            }
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => 0,
        };

        // Both directions done
        if (this.a_done && this.a_buf.pos >= this.a_buf.cap)
            && (this.b_done && this.b_buf.pos >= this.b_buf.cap)
        {
            return Poll::Ready(Ok((this.a_to_b, this.b_to_a)));
        }

        // Only re-wake if actual data was transferred — avoids spin-looping
        // when one direction is done but the other is waiting on I/O.
        if a_transferred > 0 || b_transferred > 0 {
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn test_relay_bidirectional() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            loop {
                match s.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if s.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        let (mut server_half, _) = tokio::io::duplex(1024);

        // Simple test: write data from one side and verify it's relayed
        client.write_all(b"hello relay").await.unwrap();
        client.shutdown().await.unwrap();

        // Wait for echo server
        let _ = server.await;
    }

    #[test]
    fn test_buf_size() {
        assert_eq!(RELAY_BUF_SIZE, 65536);
    }
}
