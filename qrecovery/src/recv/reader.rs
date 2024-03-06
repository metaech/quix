use super::recver::{ArcRecver, Recver};
use std::{
    io,
    ops::DerefMut,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

#[derive(Debug)]
pub struct Reader(ArcRecver);

impl Reader {
    pub(super) fn new(recver: ArcRecver) -> Self {
        Self(recver)
    }
}

// TODO: 还要实现abort
// TODO: Reader的drop，意味着自动abort

impl AsyncRead for Reader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut recver = self.0.lock().unwrap();
        let inner = recver.deref_mut();
        // 能相当清楚地看到应用层读取数据驱动的接收状态演变
        match inner {
            Ok(receiving_state) => match receiving_state.take() {
                Recver::Recv(mut r) => {
                    let result = r.poll_read(cx, buf);
                    receiving_state.replace(Recver::Recv(r));
                    result
                }
                Recver::SizeKnown(mut r) => {
                    let result = r.poll_read(cx, buf);
                    receiving_state.replace(Recver::SizeKnown(r));
                    result
                }
                Recver::DataRecvd(mut r) => {
                    r.poll_read(buf);
                    if r.is_all_read() {
                        receiving_state.replace(Recver::DataRead);
                    } else {
                        receiving_state.replace(Recver::DataRecvd(r));
                    }
                    Poll::Ready(Ok(()))
                }
                Recver::DataRead => Poll::Ready(Ok(())),
                Recver::ResetRecvd(_final_size) => {
                    receiving_state.replace(Recver::ResetRead);
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "reset by peer",
                    )))
                }
                Recver::ResetRead => {
                    receiving_state.replace(Recver::ResetRead);
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "you know, reset by peer",
                    )))
                }
            },
            Err(e) => Poll::Ready(Err(io::Error::new(e.kind(), e.to_string()))),
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let mut recver = self.0.lock().unwrap();
        let inner = recver.deref_mut();
        match inner {
            Ok(receiving_state) => match receiving_state {
                Recver::Recv(r) => {
                    r.abort();
                }
                Recver::SizeKnown(r) => {
                    r.abort();
                }
                _ => (),
            },
            Err(_) => (),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
