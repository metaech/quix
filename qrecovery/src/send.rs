use std::sync::{Arc, Mutex};

pub mod sndbuf;

mod outgoing;
mod sender;
mod writer;

pub use outgoing::{CancelTooLate, IsCancelled, Outgoing};
pub use sender::Sender;
pub use writer::Writer;

pub fn new(initial_max_stream_data: u64) -> (Outgoing, Writer) {
    let arc_sender = Arc::new(Mutex::new(Sender::with_buf_size(initial_max_stream_data)));
    let writer = Writer(arc_sender.clone());
    let outgoing = Outgoing(arc_sender);
    (outgoing, writer)
}

#[cfg(test)]
mod tests {

    #[test]
    fn it_works() {
        println!("sender::tests::it_works");
    }
}
